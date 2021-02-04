#include "branch.h"
#include "branch_type.h"
#include "variant.h"
#include "error.h"
#include "path_range.h"
#include "branch_view.h"
#include "object/tree.h"
#include "object/tagged.h"
#include "object/blob.h"
#include "refcount.h"
#include "archive.h"
#include "snapshot.h"
#include "ouisync_assert.h"

#include <boost/filesystem.hpp>
#include <boost/serialization/vector.hpp>
#include <boost/serialization/set.hpp>
#include <string.h> // memcpy

#include <iostream>

using namespace ouisync;
using std::move;
using object::Blob;
using object::Tree;
using std::unique_ptr;
using std::make_unique;
using std::string;

/* static */
Branch Branch::create(const fs::path& path, UserId user_id, ObjectStore& objects, Options::Branch options)
{
    ObjectId root_id;
    VersionVector clock;

    if (fs::exists(path)) {
        throw std::runtime_error("Local branch already exits");
    }

    object::Tree root_obj;

    root_id = objects.store(root_obj);
    objects.rc(root_id).increment_recursive_count();

    Branch branch(path, user_id, Commit{move(clock), root_id}, objects, move(options));
    branch.store_self();

    return branch;
}

/* static */
Branch Branch::load(const fs::path& file_path, UserId user_id, ObjectStore& objects, Options::Branch options)
{
    Branch branch(file_path, user_id, objects, std::move(options));
    archive::load(file_path, branch);
    return branch;
}

//--------------------------------------------------------------------
static
void decrement_rc_and_remove_single_node(ObjectStore& objstore, const ObjectId& id)
{
    auto rc = objstore.rc(id);
    rc.decrement_recursive_count_but_dont_remove();
    if (!rc.both_are_zero()) return;
    objstore.remove(id);
}

//--------------------------------------------------------------------

class Branch::Op {
    public:
    virtual Opt<Commit> commit() = 0;
    virtual ObjectStore& objstore() = 0;
    virtual ~Op() {}
};

class Branch::TreeOp : public Branch::Op {
    public:
    virtual Tree& tree() = 0;
    virtual ~TreeOp() {};
};

#define DBG std::cerr << __PRETTY_FUNCTION__ << ":" << __LINE__ << " "

class Branch::RootOp : public Branch::TreeOp {
public:
    RootOp(ObjectStore& objstore, const UserId& this_user_id, const Commit& commit) :
        _objstore(objstore),
        _this_user_id(this_user_id),
        _commit(commit)
    {
        _tree = _objstore.load<Tree>(_commit.root_id);
    }

    Tree& tree() override {
        return _tree;
    }

    Opt<Commit> commit() override {
        auto old_id = _commit.root_id;
        auto new_id = _tree.calculate_id();

        if (new_id == old_id) return boost::none;

        _objstore.store(_tree);

        for (auto id : _tree.children()) {
            objstore().rc(id).increment_recursive_count();
        }

        _objstore.rc(new_id).increment_recursive_count();
        _objstore.rc(old_id).decrement_recursive_count();

        _commit.root_id = new_id;
        _commit.stamp.increment(_this_user_id);

        return _commit;
    }

    ObjectStore& objstore() override { return _objstore; }

private:
    ObjectStore& _objstore;
    UserId _this_user_id;
    Commit _commit;
    Tree _tree;
};

class Branch::CdOp : public Branch::TreeOp {
public:
    CdOp(unique_ptr<Branch::TreeOp> parent, const UserId& this_user_id, string dirname, bool create = false) :
        _parent(move(parent)),
        _this_user_id(this_user_id),
        _dirname(move(dirname))
    {
        auto child_versions = _parent->tree().find(_dirname);

        if (!child_versions && !create) throw_error(sys::errc::no_such_file_or_directory);

        VersionVector::Version highest_version = 0u;

        for (auto& [user_id, vobj] : child_versions) {
            // XXX: It should be enough to take a version that has been completely downloaded.
            // But we don't keep that meta information yet. Thus we're using the fact here that
            // whenever a user makes a change to a node, that node must have been complete.
            if (user_id == _this_user_id) {
                _old_tree_id = vobj.object_id;
                _next_tree_vv = vobj.version_vector;
                _next_tree_vv.increment(_this_user_id);
                _tree = objstore().load<Tree>(vobj.object_id);
                return;
            }

            highest_version = std::max(highest_version, vobj.version_vector.version_of(_this_user_id));
        }

        _next_tree_vv.set_version(_this_user_id, highest_version + 1);
    }

    Tree& tree() override {
        return _tree;
    }

    Opt<Commit> commit() override {
        auto new_tree_id = _tree.calculate_id();

        if (_old_tree_id && *_old_tree_id == new_tree_id) {
            return boost::none;
        }

        _parent->tree()[_dirname][_this_user_id] = { new_tree_id, _next_tree_vv };

        objstore().store(_tree);

        for (auto id : _tree.children()) {
            objstore().rc(id).increment_recursive_count();
        }

        return _parent->commit();
    }

    ObjectStore& objstore() override {
        return _parent->objstore();
    }

private:
    unique_ptr<TreeOp> _parent;
    UserId _this_user_id;
    string _dirname;
    Opt<ObjectId> _old_tree_id;
    Tree _tree;
    VersionVector _next_tree_vv;
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
            _blob = objstore().load<Blob>(_old->blob_id);
        }
    }

    Opt<Blob>& blob() { return _blob; }

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

        vv.increment(_this_user_id);

        _parent->tree()[_filename][_this_user_id] = { new_id, move(vv) };

        return _parent->commit();
    }

    ObjectStore& objstore() override {
        return _parent->objstore();
    }

private:
    unique_ptr<TreeOp> _parent;
    UserId _this_user_id;
    string _filename;
    Opt<OldData> _old;
    Opt<Blob> _blob;
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

    ObjectStore& objstore() override {
        return _parent->objstore();
    }

private:
    unique_ptr<Branch::TreeOp> _parent;
    Tree::MutableHandle _tree_entry;
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
    return make_unique<Branch::RootOp>(_objects, _user_id, _commit);
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

void Branch::store(PathRange path, const Blob& blob)
{
    auto file = get_file(path);
    file->blob() = blob;
    commit(file);
}

void Branch::store(const fs::path& path, const Blob& blob)
{
    store(Path(path), blob);
}

//--------------------------------------------------------------------

size_t Branch::write(PathRange path, const char* buf, size_t size, size_t offset)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    auto file = get_file(path);

    if (!file->blob()) {
        file->blob() = Blob{};
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
        file->blob() = Blob{};
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
    if (!_objects.is_complete(_commit.root_id)) {
        std::cerr << "Branch is incomplete:\n";
        std::cerr << *this << "\n";
        ouisync_assert(false);
    }
}

//--------------------------------------------------------------------

Snapshot Branch::create_snapshot() const
{
    auto snapshot = Snapshot::create(_commit, _objects, _options);
    snapshot.insert_object(_commit.root_id, {});
    return snapshot;
}

//--------------------------------------------------------------------

void Branch::store_self() const {
    archive::store(_file_path, *this);
}

//--------------------------------------------------------------------

Branch::Branch(const fs::path& file_path, const UserId& user_id,
        Commit commit, ObjectStore& objects, Options::Branch options) :
    _file_path(file_path),
    _options(move(options)),
    _objects(objects),
    _user_id(user_id),
    _commit(move(commit))
{}

Branch::Branch(const fs::path& file_path,
        const UserId& user_id, ObjectStore& objects, Options::Branch options) :
    _file_path(file_path),
    _options(move(options)),
    _objects(objects),
    _user_id(user_id)
{
}

//--------------------------------------------------------------------

bool Branch::introduce_commit(const Commit& commit)
{
    if (!(_commit.stamp.same_as_or_happened_before(commit.stamp))) return false;
    if (_commit.root_id == commit.root_id) return false;

    auto old_root = _commit.root_id;

    _commit = commit;

    store_self();

    _objects.rc(_commit.root_id).increment_recursive_count();
    _objects.rc(old_root).decrement_recursive_count();

    return true;
}

//--------------------------------------------------------------------

std::ostream& ouisync::operator<<(std::ostream& os, const Branch& branch)
{
    os  << "Branch:\n";
    branch.branch_view().show(os);
    return os;
}
