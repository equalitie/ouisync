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
#include "object/refcount.h"

#include <boost/filesystem.hpp>
#include <boost/archive/binary_oarchive.hpp>
#include <boost/archive/binary_iarchive.hpp>
#include <boost/serialization/vector.hpp>
#include <string.h> // memcpy

#include <iostream>

using namespace ouisync;
using std::move;
using object::Id;
using object::Blob;
using object::Tree;
using object::JustTag;

/* static */
LocalBranch LocalBranch::create(const fs::path& path, const fs::path& objdir, UserId user_id) {
    object::Id root_id;
    VersionVector clock;

    if (fs::exists(path)) {
        throw std::runtime_error("Local branch already exits");
    }

    object::Tree root_obj;
    root_id = root_obj.store(objdir);
    LocalBranch branch(path, objdir, user_id, Commit{move(clock), root_id});
    branch.store_self();
    return branch;
}

//--------------------------------------------------------------------

static
bool _flat_remove(const fs::path& objdir, const Id& id) {
    auto rc = object::refcount::decrement(objdir, id);
    if (rc > 0) return true;
    return object::io::remove(objdir, id);
}

static
bool _remove_with_children(const fs::path& objdir, const Id& id) {
    auto obj = object::io::load<Tree, JustTag<Blob>>(objdir, id);

    apply(obj,
            [&](const Tree& tree) {
                for (auto& [name, id] : tree) {
                    (void)name; // https://stackoverflow.com/a/40714311/273348
                    _remove_with_children(objdir, id);
                }
            },
            [&](const JustTag<Blob>&) {
            });

    return _flat_remove(objdir, id);
}

template<class F>
static
Id _update_dir(size_t branch_count, const fs::path& objdir, Id tree_id, PathRange path, F&& f)
{
    Tree tree = object::io::load<Tree>(objdir, tree_id);
    auto rc = object::refcount::read(objdir, tree_id);
    assert(rc);

    Opt<Id> child_id;

    if (path.empty()) {
        child_id = f(tree, branch_count + (rc-1));
    } else {
        auto child_i = tree.find(path.front().string());

        if (child_i == tree.end()) {
            throw_error(sys::errc::no_such_file_or_directory);
        }

        path.advance_begin(1);
        child_id = _update_dir(branch_count + (rc-1), objdir, child_i->second, path, std::forward<F>(f));
        child_i->second = *child_id;
    }

    auto [new_id, created] = object::io::store_(objdir, tree);
    if (created && child_id) {
        object::refcount::increment(objdir, *child_id);
    }

    if (branch_count == 1) {
        _flat_remove(objdir, tree_id);
    }

    return new_id;
}

template<class F>
void LocalBranch::update_dir(PathRange path, F&& f)
{
    auto id = _update_dir(1, _objdir, root_id(), path, std::forward<F>(f));
    root_id(id);
}

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
            auto [i, inserted] = tree.insert(std::make_pair(path.back().native(), Id{}));
            if (!inserted) throw_error(sys::errc::file_exists);
            auto [id, created] = object::io::store_(_objdir, blob);
            i->second = id;
            return id;
        });
}

void LocalBranch::store(const fs::path& path, const Blob& blob)
{
    store(path_range(path), blob);
}

//--------------------------------------------------------------------

size_t LocalBranch::write(PathRange path, const char* buf, size_t size, size_t offset)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    update_dir(parent(path),
        [&] (Tree& tree, size_t branch_count) {
            auto i = tree.find(path.back().native());
            if (i == tree.end()) throw_error(sys::errc::no_such_file_or_directory);

            // XXX: Write only the necessary part to disk without loading
            // the whole blob into the memory.
            auto blob = object::io::load<Blob>(_objdir, i->second);

            size_t len = blob.size();

            if (offset + size > len) {
                blob.resize(offset + size);
            }

            memcpy(blob.data() + offset, buf, size);

            if (branch_count <= 1) {
                _flat_remove(_objdir, i->second);
            }

            i->second = object::io::store(_objdir, blob);
            return i->second;
        });

    return size;
}

//--------------------------------------------------------------------

size_t LocalBranch::truncate(PathRange path, size_t size)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    update_dir(parent(path),
        [&] (Tree& tree, auto branch_count) {
            auto i = tree.find(path.back().native());
            if (i == tree.end()) throw_error(sys::errc::no_such_file_or_directory);

            // XXX: Read only what's needed, not the whole blob
            auto blob = object::io::load<Blob>(_objdir, i->second);

            blob.resize(std::min<size_t>(blob.size(), size));
            size = blob.size();

            if (branch_count <= 1) {
                _flat_remove(_objdir, i->second);
            }

            i->second = object::io::store(_objdir, blob);
            return i->second;
        });

    return size;
}

//--------------------------------------------------------------------

void LocalBranch::mkdir(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::invalid_argument);

    update_dir(parent(path),
        [&] (Tree& parent, auto) {
            auto [i, inserted] = parent.insert(std::make_pair(path.back().native(), Id{}));
            if (!inserted) throw_error(sys::errc::file_exists);
            auto [id, created] = object::io::store_(_objdir, Tree{});
            i->second = id;
            return id;
        });
}

//--------------------------------------------------------------------

bool LocalBranch::remove(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::operation_not_permitted);

    update_dir(parent(path),
        [&] (Tree& tree, size_t branch_count) {
            auto i = tree.find(path.back().native());
            if (i == tree.end()) throw_error(sys::errc::no_such_file_or_directory);
            if (branch_count <= 1) {
                _remove_with_children(_objdir, i->second);
            }
            tree.erase(i);
            return boost::none;
        });

    return true;
}

bool LocalBranch::remove(const fs::path& path)
{
    return remove(path_range(path));
}

//--------------------------------------------------------------------

void LocalBranch::store_self() const {
    fs::fstream file(_file_path, file.binary | file.trunc | file.out);

    if (!file.is_open())
        throw std::runtime_error("Failed to open branch file");

    boost::archive::binary_oarchive oa(file);

    store_tag(oa);
    store_rest(oa);

    object::refcount::increment(_objdir, _root_id);
}

//--------------------------------------------------------------------

void LocalBranch::root_id(const object::Id& id) {
    if (_root_id == id) return;
    _root_id = id;
    _stamp.increment(_user_id);
    store_self();
}

//--------------------------------------------------------------------

LocalBranch::LocalBranch(const fs::path& file_path, const fs::path& objdir,
        const UserId& user_id, Commit commit) :
    _file_path(file_path),
    _objdir(objdir),
    _user_id(user_id),
    _root_id(commit.root_id),
    _stamp(move(commit.stamp))
{}

LocalBranch::LocalBranch(const fs::path& file_path, const fs::path& objdir, IArchive& ar) :
    _file_path(file_path),
    _objdir(objdir)
{
    load_rest(ar);
}

//--------------------------------------------------------------------

object::Id LocalBranch::id_of(PathRange path) const
{
    return immutable_io().id_of(path);
}

//--------------------------------------------------------------------

void LocalBranch::store_tag(OArchive& ar) const
{
    ar << BranchType::Local;
}

void LocalBranch::store_rest(OArchive& ar) const
{
    ar << _user_id;
    ar << _root_id;
    ar << _stamp;
}

void LocalBranch::load_rest(IArchive& ar)
{
    ar >> _user_id;
    ar >> _root_id;
    ar >> _stamp;
}

//--------------------------------------------------------------------

std::ostream& ouisync::operator<<(std::ostream& os, const LocalBranch& branch)
{
    branch.immutable_io().show(os);
    return os;
}
