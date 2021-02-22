#include "branch.h"
#include "variant.h"
#include "error.h"
#include "path_range.h"
#include "branch_view.h"
#include "archive.h"
#include "ouisync_assert.h"
#include "multi_dir.h"

#include <boost/filesystem.hpp>
#include <boost/serialization/vector.hpp>
#include <boost/serialization/set.hpp>
#include <string.h> // memcpy

#include <iostream>

using namespace ouisync;
using std::move;
using std::unique_ptr;
using std::make_unique;
using std::string;
using std::set;
using std::cerr;

#define DBG std::cerr << __PRETTY_FUNCTION__ << ":" << __LINE__ << " "

/* static */
Branch Branch::create(executor_type ex, const fs::path& path, UserId user_id, ObjectStore& objstore, Options::Branch options)
{
    if (fs::exists(path)) {
        throw std::runtime_error("Local branch already exits");
    }

    Branch b(ex, path, user_id, objstore, move(options));

    auto empty_dir_id = objstore.store(Directory{});

    b._index = Index(user_id, {empty_dir_id, {}});

    b.store_self();

    return b;
}

/* static */
Branch Branch::load(executor_type ex, const fs::path& file_path, UserId user_id, ObjectStore& objstore, Options::Branch options)
{
    Branch b(ex, file_path, user_id, objstore, std::move(options));
    archive::load(file_path, b);
    return b;
}

//--------------------------------------------------------------------

Branch::Branch(executor_type ex, const fs::path& file_path, const UserId& user_id,
        ObjectStore& objstore, Options::Branch options) :
    _ex(ex),
    _file_path(file_path),
    _options(move(options)),
    _objstore(objstore),
    _user_id(user_id),
    _state_change_wait(_ex)
{
}

//--------------------------------------------------------------------

class Branch::Op {
    public:
    virtual bool commit() = 0;
    virtual Branch::RootOp* root() = 0;
    virtual ~Op() {}
};

class Branch::TreeOp : public Branch::Op {
    public:
    virtual Directory& tree() = 0;
    virtual const MultiDir& multi_dir() const = 0;
    virtual ~TreeOp() {};
};

class Branch::RootOp : public Branch::TreeOp {
public:
    RootOp(ObjectStore& objstore, const UserId& this_user_id, Index& index) :
        _objstore(objstore),
        _this_user_id(this_user_id),
        _index(index),
        _original_commit(*_index.commit(this_user_id)),
        _multi_dir(_index.commits(), objstore)
    {
        _tree = _objstore.load<Directory>(_original_commit.id);
    }

    Directory& tree() override {
        return _tree;
    }

    bool commit() override {
        auto new_id = _tree.calculate_id();
        auto old_id = _original_commit.id;

        if (old_id == new_id) return false;

        _objstore.store(_tree);

        _tree.for_each_unique_child([&] (auto& filename, auto& child_id) {
                _index.insert_object(_this_user_id, child_id, new_id);
            });

        _index.insert_object(_this_user_id, new_id, new_id);

        _index.set_version_vector(_this_user_id, _tree.calculate_version_vector_union());

        remove_recursive(old_id, old_id);

        return true;
    }

    Index& index() { return _index; }
    ObjectStore& objstore() { return _objstore; }

    RootOp* root() override { return this; }

    void increment(VersionVector& vv) const {
        vv.set_version(_this_user_id, _original_commit.versions.version_of(_this_user_id) + 1);
    }

    void remove_recursive(const ObjectId& obj_id, const ObjectId& parent_id) {
        _index.remove_object(_this_user_id, obj_id, parent_id);

        if (_index.someone_has(obj_id)) return;

        auto obj = _objstore.load<Directory, FileBlob::Nothing>(obj_id);

        apply(obj,
                [&](const Directory& d) {
                    d.for_each_unique_child([&] (auto& filename, auto& child_id) {
                        remove_recursive(child_id, obj_id);
                    });
                },
                [&](const FileBlob::Nothing&) {
                });

        _objstore.remove(obj_id);
    }

    const MultiDir& multi_dir() const override { return _multi_dir; }

private:
    ObjectStore& _objstore;
    UserId _this_user_id;
    Directory _tree;
    Index& _index;
    VersionedObject _original_commit;
    MultiDir _multi_dir;
};

class Branch::CdOp : public Branch::TreeOp {
public:
    CdOp(unique_ptr<Branch::TreeOp> parent, const UserId& this_user_id, string dirname, bool create = false) :
        _parent(move(parent)),
        _root(_parent->root()),
        _this_user_id(this_user_id),
        _dirname(move(dirname)),
        _multi_dir(_parent->multi_dir().cd_into(_dirname))
    {
        auto opt_user_version = _multi_dir.pick_subdirectory_to_edit(this_user_id, _dirname);

        if (!opt_user_version) {
            if (!create) throw_error(sys::errc::no_such_file_or_directory);
            return; // One will be created in `commit()`
        }

        _old = opt_user_version->vobj;

        _tree = objstore().load<Directory>(_old->id);
    }

    Directory& tree() override {
        return _tree;
    }

    bool commit() override {
        auto new_tree_id = _tree.calculate_id();

        if (_old && _old->id == new_tree_id) {
            return false;
        }

        VersionVector new_vv;

        if (_old) {
            // XXX: We should make a union of all `_dirname`s per each users
            // that we've have downloaded *completely*.
            new_vv = _old->versions;
        }

        root()->increment(new_vv);

        _parent->tree()[_dirname][_this_user_id] = { new_tree_id, move(new_vv) };

        objstore().store(_tree);

        _tree.for_each_unique_child([&] (auto& filename, auto& child_id) {
                _root->index().insert_object(_this_user_id, child_id, new_tree_id);
            });

        return _parent->commit();
    }

    ObjectStore& objstore() {
        return _root->objstore();
    }

    RootOp* root() override { return _root; }

    const MultiDir& multi_dir() const override { return _multi_dir; }

private:
    unique_ptr<TreeOp> _parent;
    RootOp* _root;
    UserId _this_user_id;
    string _dirname;
    Opt<VersionedObject> _old;
    Directory _tree;
    MultiDir _multi_dir;
};

class Branch::FileOp : public Branch::Op {
public:
    FileOp(unique_ptr<TreeOp> parent, const UserId& this_user_id, string filename) :
        _parent(move(parent)),
        _this_user_id(this_user_id),
        _filename(move(filename))
    {
        auto per_name = _parent->tree().find(_filename);

        if (!per_name) {
            return;
        }

        auto i = per_name.find(_this_user_id);

        if (i != per_name.end()) {
            _old = i->second;
            _blob = objstore().load<FileBlob>(_old->id);
        }
    }

    Opt<FileBlob>& blob() { return _blob; }

    bool commit() override {
        if (!_blob && !_old) return false;

        if (!_blob) { // Was removed
            // We need a mark that the file was removed.
            assert("TODO" && 0);
        }

        auto new_id = _blob->calculate_id();

        if (_old && _old->id == new_id) return false;

        objstore().store(*_blob);

        VersionVector vv;

        if (_old) {
            vv = _old->versions;
        }

        root()->increment(vv);

        _parent->tree()[_filename][_this_user_id] = { new_id, move(vv) };

        return _parent->commit();
    }

    ObjectStore& objstore() {
        return root()->objstore();
    }

    RootOp* root() override { return _parent->root(); }

private:
    unique_ptr<TreeOp> _parent;
    UserId _this_user_id;
    string _filename;
    Opt<VersionedObject> _old;
    Opt<FileBlob> _blob;
};

// XXX: This is just a stub, proper implementation needs to create an entry
// that marks the file/directory as removed. Otherwise concurrent edits would
// always re-add that directory back.
class Branch::RemoveOp : public Branch::Op {
public:
    RemoveOp(unique_ptr<Branch::TreeOp> parent, string filename) :
        _parent(move(parent))
    {
        _tree_entry = _parent->tree().find(filename);

        if (!_tree_entry) {
            throw_error(sys::errc::no_such_file_or_directory);
        }
    }

    bool commit() override {
        _parent->tree().erase(_tree_entry);
        return _parent->commit();
    }

    RootOp* root() override { return _parent->root(); }

private:
    unique_ptr<Branch::TreeOp> _parent;
    Directory::MutableHandle _tree_entry;
};

//--------------------------------------------------------------------

static
PathRange parent(PathRange path) {
    path.advance_end(-1);
    return path;
}

//--------------------------------------------------------------------
template<class OpPtr>
void Branch::do_commit(OpPtr& op)
{
    if (op->commit()) {
        _state_change_wait.notify();
    }
}

//--------------------------------------------------------------------
unique_ptr<Branch::TreeOp> Branch::root_op()
{
    return make_unique<Branch::RootOp>(_objstore, _user_id, _index);
}

MultiDir Branch::root_multi_dir() const
{
    return MultiDir(_index.commits(), _objstore);
}

unique_ptr<Branch::TreeOp> Branch::cd_into(PathRange path)
{
    unique_ptr<TreeOp> dir = root_op();

    for (auto& p : path) {
        dir = make_unique<Branch::CdOp>(move(dir), _user_id, p);
    }

    return dir;
}

unique_ptr<Branch::FileOp> Branch::get_file(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    unique_ptr<TreeOp> dir = cd_into(parent(path));

    return make_unique<FileOp>(move(dir), _user_id, path.back());
}

//--------------------------------------------------------------------
set<string> Branch::readdir(PathRange path) const
{
    MultiDir dir = root_multi_dir().cd_into(path);
    return dir.list();
}

//--------------------------------------------------------------------

void Branch::store(PathRange path, const FileBlob& blob)
{
    auto file = get_file(path);
    file->blob() = blob;
    do_commit(file);
}

void Branch::store(const fs::path& path, const FileBlob& blob)
{
    store(Path(path), blob);
}

//--------------------------------------------------------------------

size_t Branch::write(PathRange path, const char* buf, size_t size, size_t offset)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    auto file = get_file(path);

    if (!file->blob()) {
        file->blob() = FileBlob{};
    }

    auto& blob = *file->blob();

    size_t len = blob.size();

    if (offset + size > len) {
        blob.resize(offset + size);
    }

    memcpy(blob.data() + offset, buf, size);

    do_commit(file);

    return size;
}

//--------------------------------------------------------------------

size_t Branch::truncate(PathRange path, size_t size)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    auto file = get_file(path);

    if (!file->blob()) {
        file->blob() = FileBlob{};
    }

    auto& blob = *file->blob();

    blob.resize(std::min<size_t>(blob.size(), size));

    do_commit(file);

    return blob.size();
}

//--------------------------------------------------------------------

void Branch::mkdir(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::invalid_argument);

    unique_ptr<TreeOp> dir = cd_into(parent(path));

    if (dir->tree().find(path.back())) throw_error(sys::errc::file_exists);

    dir = make_unique<Branch::CdOp>(move(dir), _user_id, path.back(), true);

    do_commit(dir);
}

//--------------------------------------------------------------------

bool Branch::remove(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::operation_not_permitted);

    auto dir = cd_into(parent(path));
    auto rm = make_unique<Branch::RemoveOp>(move(dir), path.back());

    do_commit(rm);
    return true;
}

bool Branch::remove(const fs::path& fspath)
{
    Path path(fspath);
    return remove(path);
}

//--------------------------------------------------------------------

void Branch::merge_index(const Index& index)
{
    _index.merge(index, _objstore);
}

//--------------------------------------------------------------------

void Branch::store_self() const {
    archive::store(_file_path, *this);
}

//--------------------------------------------------------------------

BranchView Branch::branch_view() const {
    return BranchView(_objstore, _index.commits());
}

//--------------------------------------------------------------------

std::ostream& ouisync::operator<<(std::ostream& os, const Branch& branch)
{
    os  << "Branch:\n";
    branch.branch_view().show(os);
    return os;
}
