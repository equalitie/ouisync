#include "local_branch.h"
#include "branch_type.h"
#include "variant.h"
#include "error.h"
#include "path_range.h"
#include "branch_view.h"
#include "object/tree.h"
#include "object/tagged.h"
#include "object/blob.h"
#include "object/io.h"
#include "refcount.h"
#include "archive.h"
#include "snapshot.h"
#include "ouisync_assert.h"

#include <boost/filesystem.hpp>
#include <boost/serialization/vector.hpp>
#include <string.h> // memcpy

#include <iostream>

using namespace ouisync;
using std::move;
using object::Blob;
using object::Tree;

/* static */
LocalBranch LocalBranch::create(const fs::path& path, UserId user_id, ObjectStore& objects, Options::LocalBranch options)
{
    ObjectId root_id;
    VersionVector clock;

    if (fs::exists(path)) {
        throw std::runtime_error("Local branch already exits");
    }

    object::Tree root_obj;

    root_id = objects.store(root_obj);
    objects.rc(root_id).increment_recursive_count();

    LocalBranch branch(path, user_id, Commit{move(clock), root_id}, objects, move(options));
    branch.store_self();

    return branch;
}

/* static */
LocalBranch LocalBranch::load(const fs::path& file_path, UserId user_id, ObjectStore& objects, Options::LocalBranch options)
{
    LocalBranch branch(file_path, user_id, objects, std::move(options));
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

template<class F>
static
ObjectId _update_dir(size_t branch_count, ObjectStore& objstore, ObjectId tree_id, PathRange path, F&& f)
{
    Tree tree = objstore.load<Tree>(tree_id);
    auto rc = objstore.rc(tree_id).recursive_count();
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
        new_child_id = _update_dir(branch_count + (rc-1), objstore, child.id(), path, std::forward<F>(f));
        child.set_id(*new_child_id);
    }

    auto [new_id, created] = objstore.store_(tree);

    if (created && new_child_id) {
        objstore.rc(*new_child_id).increment_recursive_count();
    }

    if (branch_count == 1) {
        decrement_rc_and_remove_single_node(objstore, tree_id);
    }

    return new_id;
}

template<class F>
void LocalBranch::update_dir(PathRange path, F&& f)
{
    auto id = _update_dir(1, _objects, _commit.root_id, path, std::forward<F>(f));

    if (_commit.root_id == id) return;

    _commit.root_id = id;
    _commit.stamp.increment(_user_id);

    store_self();

    _objects.rc(_commit.root_id).increment_recursive_count();
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
            auto [id, created] = _objects.store_(blob);
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
            auto blob = _objects.load<Blob>(child.id());

            size_t len = blob.size();

            if (offset + size > len) {
                blob.resize(offset + size);
            }

            memcpy(blob.data() + offset, buf, size);

            if (branch_count <= 1) {
                decrement_rc_and_remove_single_node(_objects, child.id());
            }

            child.set_id(_objects.store(blob));
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
            auto blob = _objects.load<Blob>(child.id());

            blob.resize(std::min<size_t>(blob.size(), size));
            size = blob.size();

            if (branch_count <= 1) {
                decrement_rc_and_remove_single_node(_objects, child.id());
            }

            child.set_id(_objects.store(blob));
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
            auto [id, created] = _objects.store_(Tree{});
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
                _objects.rc(child.id()).decrement_recursive_count();
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
    if (!_objects.is_complete(_commit.root_id)) {
        std::cerr << "LocalBranch is incomplete:\n";
        std::cerr << *this << "\n";
        ouisync_assert(false);
    }
}

//--------------------------------------------------------------------

Snapshot LocalBranch::create_snapshot() const
{
    auto snapshot = Snapshot::create(_commit, _objects, _options);
    snapshot.insert_object(_commit.root_id, {});
    return snapshot;
}

//--------------------------------------------------------------------

void LocalBranch::store_self() const {
    archive::store(_file_path, *this);
}

//--------------------------------------------------------------------

LocalBranch::LocalBranch(const fs::path& file_path, const UserId& user_id,
        Commit commit, ObjectStore& objects, Options::LocalBranch options) :
    _file_path(file_path),
    _options(move(options)),
    _objects(objects),
    _user_id(user_id),
    _commit(move(commit))
{}

LocalBranch::LocalBranch(const fs::path& file_path,
        const UserId& user_id, ObjectStore& objects, Options::LocalBranch options) :
    _file_path(file_path),
    _options(move(options)),
    _objects(objects),
    _user_id(user_id)
{
}

//--------------------------------------------------------------------

bool LocalBranch::introduce_commit(const Commit& commit)
{
    if (!(_commit.stamp <= commit.stamp)) return false;
    if (_commit.root_id == commit.root_id) return false;

    auto old_root = _commit.root_id;

    _commit = commit;

    store_self();

    _objects.rc(_commit.root_id).increment_recursive_count();
    _objects.rc(old_root).decrement_recursive_count();

    return true;
}

//--------------------------------------------------------------------

ObjectId LocalBranch::id_of(PathRange path) const
{
    return branch_view().id_of(path);
}

//--------------------------------------------------------------------

std::ostream& ouisync::operator<<(std::ostream& os, const LocalBranch& branch)
{
    os  << "LocalBranch:\n";
    branch.branch_view().show(os);
    return os;
}
