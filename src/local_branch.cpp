#include "local_branch.h"
#include "branch_type.h"
#include "variant.h"
#include "error.h"
#include "path_range.h"
#include "branch_io.h"
#include "object/tree.h"
#include "object/tagged.h"
#include "object/blob.h"
#include "object/io.h"
#include "refcount.h"
#include "archive.h"
#include "snapshot.h"

#include <boost/filesystem.hpp>
#include <boost/serialization/vector.hpp>
#include <string.h> // memcpy

#include <iostream>

using namespace ouisync;
using std::move;
using object::Blob;
using object::Tree;

/* static */
LocalBranch LocalBranch::create(const fs::path& path, const fs::path& objdir, UserId user_id)
{
    ObjectId root_id;
    VersionVector clock;

    if (fs::exists(path)) {
        throw std::runtime_error("Local branch already exits");
    }

    object::Tree root_obj;

    root_id = object::io::store(objdir, root_obj);
    refcount::increment_recursive(objdir, root_id);

    LocalBranch branch(path, objdir, user_id, Commit{move(clock), root_id});
    branch.store_self();

    return branch;
}

/* static */
LocalBranch LocalBranch::load(const fs::path& file_path, const fs::path& objdir, UserId user_id)
{
    LocalBranch branch(file_path, objdir, user_id);
    archive::load(file_path, branch);
    return branch;
}

//--------------------------------------------------------------------
static
void decrement_rc_and_remove_single_node(const fs::path& objdir, const ObjectId& id)
{
    auto rc = Rc::load(objdir, id);
    rc.decrement_recursive_count();
    if (!rc.both_are_zero()) return;
    object::io::remove(objdir, id);
}

//--------------------------------------------------------------------

template<class F>
static
ObjectId _update_dir(size_t branch_count, const fs::path& objdir, ObjectId tree_id, PathRange path, F&& f)
{
    Tree tree = object::io::load<Tree>(objdir, tree_id);
    auto rc = refcount::read_recursive(objdir, tree_id);
    assert(rc > 0);

    Opt<ObjectId> new_child_id;

    if (path.empty()) {
        new_child_id = f(tree, branch_count + (rc-1));
    } else {
        auto child = tree.find(path.front());

        if (!child) {
            throw_error(sys::errc::no_such_file_or_directory);
        }

        path.advance_begin(1);
        new_child_id = _update_dir(branch_count + (rc-1), objdir, child.id(), path, std::forward<F>(f));
        child.set_id(*new_child_id);
    }

    auto [new_id, created] = object::io::store_(objdir, tree);
    if (created && new_child_id) {
        refcount::increment_recursive(objdir, *new_child_id);
    }

    if (branch_count == 1) {
        decrement_rc_and_remove_single_node(objdir, tree_id);
    }

    return new_id;
}

template<class F>
void LocalBranch::update_dir(PathRange path, F&& f)
{
    auto id = _update_dir(1, _objdir, _commit.root_id, path, std::forward<F>(f));

    if (_commit.root_id == id) return;

    _commit.root_id = id;
    _commit.stamp.increment(_user_id);

    store_self();

    refcount::increment_recursive(_objdir, _commit.root_id);
}

//--------------------------------------------------------------------

static
PathRange parent(PathRange path) {
    path.advance_end(-1);
    return path;
}

//--------------------------------------------------------------------

void LocalBranch::store(PathRange path, const Blob& blob)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    update_dir(parent(path),
        [&] (Tree& tree, auto) {
            auto [child, inserted] = tree.insert(std::make_pair(path.back(), ObjectId{}));
            if (!inserted) throw_error(sys::errc::file_exists);
            auto [id, created] = object::io::store_(_objdir, blob);
            child.set_id(id);
            return id;
        });
}

void LocalBranch::store(const fs::path& path, const Blob& blob)
{
    store(Path(path), blob);
}

//--------------------------------------------------------------------

size_t LocalBranch::write(PathRange path, const char* buf, size_t size, size_t offset)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    update_dir(parent(path),
        [&] (Tree& tree, size_t branch_count) {
            auto child = tree.find(path.back());
            if (!child) throw_error(sys::errc::no_such_file_or_directory);

            // XXX: Write only the necessary part to disk without loading
            // the whole blob into the memory.
            auto blob = object::io::load<Blob>(_objdir, child.id());

            size_t len = blob.size();

            if (offset + size > len) {
                blob.resize(offset + size);
            }

            memcpy(blob.data() + offset, buf, size);

            if (branch_count <= 1) {
                decrement_rc_and_remove_single_node(_objdir, child.id());
            }

            child.set_id(object::io::store(_objdir, blob));
            return child.id();
        });

    return size;
}

//--------------------------------------------------------------------

size_t LocalBranch::truncate(PathRange path, size_t size)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    update_dir(parent(path),
        [&] (Tree& tree, auto branch_count) {
            auto child = tree.find(path.back());
            if (!child) throw_error(sys::errc::no_such_file_or_directory);

            // XXX: Read only what's needed, not the whole blob
            auto blob = object::io::load<Blob>(_objdir, child.id());

            blob.resize(std::min<size_t>(blob.size(), size));
            size = blob.size();

            if (branch_count <= 1) {
                decrement_rc_and_remove_single_node(_objdir, child.id());
            }

            child.set_id(object::io::store(_objdir, blob));
            return child.id();
        });

    return size;
}

//--------------------------------------------------------------------

void LocalBranch::mkdir(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::invalid_argument);

    update_dir(parent(path),
        [&] (Tree& parent, auto) {
            auto [child, inserted] = parent.insert(std::make_pair(path.back(), ObjectId{}));
            if (!inserted) throw_error(sys::errc::file_exists);
            auto [id, created] = object::io::store_(_objdir, Tree{});
            child.set_id(id);
            return id;
        });
}

//--------------------------------------------------------------------

bool LocalBranch::remove(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::operation_not_permitted);

    update_dir(parent(path),
        [&] (Tree& tree, size_t branch_count) {
            auto child = tree.find(path.back());
            if (!child) throw_error(sys::errc::no_such_file_or_directory);
            if (branch_count <= 1) {
                refcount::deep_remove(_objdir, child.id());
            }
            tree.erase(child);
            return boost::none;
        });

    return true;
}

bool LocalBranch::remove(const fs::path& fspath)
{
    Path path(fspath);
    return remove(path);
}

//--------------------------------------------------------------------
void LocalBranch::sanity_check() const {
    if (!object::io::is_complete(_objdir, _commit.root_id)) {
        std::cerr << *this << "\n";
        assert(0 && "LocalBranch is incomplete\n");
    }
}

//--------------------------------------------------------------------

Snapshot LocalBranch::create_snapshot(const fs::path& snapshotdir) const
{
    auto snapshot = Snapshot::create(snapshotdir, _objdir, _commit);
    snapshot.capture_object(_commit.root_id);
    return snapshot;
}

//--------------------------------------------------------------------

void LocalBranch::store_self() const {
    archive::store(_file_path, *this);
}

//--------------------------------------------------------------------

LocalBranch::LocalBranch(const fs::path& file_path, const fs::path& objdir,
        const UserId& user_id, Commit commit) :
    _file_path(file_path),
    _objdir(objdir),
    _user_id(user_id),
    _commit(move(commit))
{}

LocalBranch::LocalBranch(const fs::path& file_path, const fs::path& objdir,
        const UserId& user_id) :
    _file_path(file_path),
    _objdir(objdir),
    _user_id(user_id)
{
}

//--------------------------------------------------------------------

bool LocalBranch::introduce_commit(const Commit& commit)
{
    if (!(_commit.stamp <= commit.stamp)) return false;
    if (_commit.root_id == commit.root_id) return false;

    auto old_root = _commit.root_id;
    //auto rc = refcount::decrement(_objdir, old_root);

    _commit = move(commit);

    store_self();
    refcount::increment_recursive(_objdir, _commit.root_id);
    refcount::deep_remove(_objdir, old_root);

    return true;
}

//--------------------------------------------------------------------

ObjectId LocalBranch::id_of(PathRange path) const
{
    return immutable_io().id_of(path);
}

//--------------------------------------------------------------------

std::ostream& ouisync::operator<<(std::ostream& os, const LocalBranch& branch)
{
    os  << "LocalBranch:\n";
    branch.immutable_io().show(os);
    return os;
}
