#include "branch.h"
#include "variant.h"
#include "error.h"
#include "path_range.h"
#include "branch_view.h"
#include "archive.h"
#include "ouisync_assert.h"

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

#define DBG std::cerr << __PRETTY_FUNCTION__ << ":" << __LINE__ << " "

/* static */
Branch Branch::create(const fs::path& path, UserId user_id, ObjectStore& objstore, Options::Branch options)
{
    ObjectId root_id;
    VersionVector clock;

    if (fs::exists(path)) {
        throw std::runtime_error("Local branch already exits");
    }

    Directory root_obj;

    root_id = objstore.store(root_obj);

    Branch branch(path, user_id, Commit{move(clock), root_id}, objstore, move(options));
    branch.store_self();

    return branch;
}

/* static */
Branch Branch::load(const fs::path& file_path, UserId user_id, ObjectStore& objstore, Options::Branch options)
{
    Branch branch(file_path, user_id, objstore, std::move(options));
    archive::load(file_path, branch);
    return branch;
}

//--------------------------------------------------------------------

class Branch::Op {
    public:
    virtual Opt<Commit> commit() = 0;
    virtual Branch::RootOp* root() = 0;
    virtual ~Op() {}
};

class Branch::TreeOp : public Branch::Op {
    public:
    virtual Directory& tree() = 0;
    virtual ~TreeOp() {};
};

class Branch::RootOp : public Branch::TreeOp {
public:
    RootOp(ObjectStore& objstore, const UserId& this_user_id, const Commit& commit, Index& index) :
        _objstore(objstore),
        _this_user_id(this_user_id),
        _commit(commit),
        _index(index)
    {
        _tree = _objstore.load<Directory>(_commit.root_id);
    }

    Directory& tree() override {
        return _tree;
    }

    Opt<Commit> commit() override {
        auto old_id = _commit.root_id;
        auto new_id = _tree.calculate_id();

        if (new_id == old_id) return boost::none;

        _objstore.store(_tree);

        _tree.for_each_unique_child([&] (auto& filename, auto& child_id) {
                _index.insert_object(child_id, filename, new_id);
            });

        _index.set_root(new_id);

        _commit.root_id = new_id;

        auto new_stamp = _tree.calculate_version_vector_union();

        assert(new_stamp.happened_after(_commit.stamp));

        _commit.stamp = move(new_stamp);

        return _commit;
    }

    Index& index() { return _index; }
    ObjectStore& objstore() { return _objstore; }

    RootOp* root() override { return this; }

    void increment(VersionVector& vv) const {
        vv.set_version(_this_user_id, _commit.stamp.version_of(_this_user_id) + 1);
    }

private:
    ObjectStore& _objstore;
    UserId _this_user_id;
    Commit _commit;
    Directory _tree;
    Index& _index;
};

class Branch::CdOp : public Branch::TreeOp {
    struct Old {
        ObjectId tree_id;
        VersionVector tree_vv;
    };
public:
    CdOp(unique_ptr<Branch::TreeOp> parent, const UserId& this_user_id, string dirname, bool create = false) :
        _parent(move(parent)),
        _root(_parent->root()),
        _this_user_id(this_user_id),
        _dirname(move(dirname))
    {
        auto child_versions = _parent->tree().find(_dirname);

        if (!child_versions && !create) throw_error(sys::errc::no_such_file_or_directory);

        for (auto& [user_id, vobj] : child_versions) {
            // XXX: It should be enough to take a version that has been completely downloaded.
            // But we don't keep that meta information yet. Thus we're using the fact here that
            // whenever a user makes a change to a node, that node must have been complete.
            if (user_id == _this_user_id) {
                _old = Old{ vobj.object_id, vobj.version_vector };
                _tree = objstore().load<Directory>(vobj.object_id);
                return;
            }
        }
    }

    Directory& tree() override {
        return _tree;
    }

    Opt<Commit> commit() override {
        auto new_tree_id = _tree.calculate_id();

        if (_old && _old->tree_id == new_tree_id) {
            return boost::none;
        }

        VersionVector new_vv;

        if (_old) {
            // XXX: We should make a union of all `_dirname`s per each users
            // that we've have downloaded *completely*.
            new_vv = _old->tree_vv;
        }

        root()->increment(new_vv);

        _parent->tree()[_dirname][_this_user_id] = { new_tree_id, move(new_vv) };

        objstore().store(_tree);

        _tree.for_each_unique_child([&] (auto& filename, auto& child_id) {
                _root->index().insert_object(child_id, filename, new_tree_id);
            });

        return _parent->commit();
    }

    ObjectStore& objstore() {
        return _root->objstore();
    }

    RootOp* root() override { return _root; }

private:
    unique_ptr<TreeOp> _parent;
    RootOp* _root;
    UserId _this_user_id;
    string _dirname;
    Opt<Old> _old;
    Directory _tree;
};

class Branch::FileOp : public Branch::Op {
private:
    struct OldData {
        ObjectId blob_id;
        VersionVector version_vector;
    };

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
            _old = OldData { i->second.object_id, i->second.version_vector };
            _blob = objstore().load<FileBlob>(_old->blob_id);
        }
    }

    Opt<FileBlob>& blob() { return _blob; }

    Opt<Commit> commit() override {
        if (!_blob && !_old) return boost::none;

        if (!_blob) { // Was removed
            // We need a mark that the file was removed.
            assert("TODO" && 0);
        }

        auto new_id = _blob->calculate_id();

        if (_old && _old->blob_id == new_id) return boost::none;

        objstore().store(*_blob);

        VersionVector vv;

        if (_old) {
            vv = _old->version_vector;
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
    Opt<OldData> _old;
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

    Opt<Commit> commit() override {
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
unique_ptr<Branch::TreeOp> Branch::root()
{
    return make_unique<Branch::RootOp>(_objstore, _user_id, _commit, _index);
}

unique_ptr<Branch::TreeOp> Branch::cd_into(PathRange path)
{
    unique_ptr<TreeOp> dir = root();

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

template<class OpT>
void Branch::commit(const unique_ptr<OpT>& op)
{
    if (auto commit = op->commit()) {
        _commit = *commit;
    }
}

//--------------------------------------------------------------------

void Branch::store(PathRange path, const FileBlob& blob)
{
    auto file = get_file(path);
    file->blob() = blob;
    commit(file);
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

    commit(file);

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

    commit(file);

    return blob.size();
}

//--------------------------------------------------------------------

void Branch::mkdir(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::invalid_argument);

    unique_ptr<TreeOp> dir = cd_into(parent(path));

    if (dir->tree().find(path.back())) throw_error(sys::errc::file_exists);

    dir = make_unique<Branch::CdOp>(move(dir), _user_id, path.back(), true);

    commit(dir);
}

//--------------------------------------------------------------------

bool Branch::remove(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::operation_not_permitted);

    auto dir = cd_into(parent(path));
    auto rm = make_unique<Branch::RemoveOp>(move(dir), path.back());

    commit(rm);
    return true;
}

bool Branch::remove(const fs::path& fspath)
{
    Path path(fspath);
    return remove(path);
}

//--------------------------------------------------------------------
void Branch::sanity_check() const {
    if (!_objstore.is_complete(_commit.root_id)) {
        std::cerr << "Branch is incomplete:\n";
        std::cerr << *this << "\n";
        ouisync_assert(false);
    }
}

//--------------------------------------------------------------------

void Branch::store_self() const {
    archive::store(_file_path, *this);
}

//--------------------------------------------------------------------

Branch::Branch(const fs::path& file_path, const UserId& user_id,
        Commit commit, ObjectStore& objstore, Options::Branch options) :
    _file_path(file_path),
    _options(move(options)),
    _objstore(objstore),
    _user_id(user_id),
    _commit(move(commit)),
    _index(_objstore)
{
    _index.set_root(_commit.root_id);
}

Branch::Branch(const fs::path& file_path,
        const UserId& user_id, ObjectStore& objstore, Options::Branch options) :
    _file_path(file_path),
    _options(move(options)),
    _objstore(objstore),
    _user_id(user_id),
    _index(_objstore)
{
}

//--------------------------------------------------------------------

std::ostream& ouisync::operator<<(std::ostream& os, const Branch& branch)
{
    os  << "Branch:\n";
    branch.branch_view().show(os);
    return os;
}
