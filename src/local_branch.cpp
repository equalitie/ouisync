#include "local_branch.h"
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
    object::refcount::increment(objdir, root_id);
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

template<class Obj>
static
Id _flat_store(const fs::path& objdir, const Obj& obj) {
    auto new_id = object::io::store(objdir, obj);
    object::refcount::increment(objdir, new_id);
    return new_id;
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
Opt<Id> _update_dir(const fs::path& objdir, Id tree_id, PathRange path, F&& f)
{
    Tree tree = object::io::load<Tree>(objdir, tree_id);

    if (path.empty()) {
        auto updated = f(tree);
        if (!updated) return boost::none;
    } else {
        auto child_i = tree.find(path.front().string());

        if (child_i == tree.end()) {
            throw_error(sys::errc::no_such_file_or_directory);
        }

        path.advance_begin(1);
        auto oid = _update_dir(objdir, child_i->second, path, std::forward<F>(f));
        if (!oid) return oid;
        child_i->second = *oid;
    }

    _flat_remove(objdir, tree_id);
    return _flat_store(objdir, tree);
}

template<class F>
void LocalBranch::update_dir(PathRange path, F&& f)
{
    auto oid = _update_dir(_objdir, root_object_id(), path, std::forward<F>(f));

    if (oid) {
        root_object_id(*oid);
    }
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
        [&] (Tree& tree) {
            auto [child_i, inserted] = tree.insert(std::make_pair(path.back().native(), Id{}));
            if (!inserted) throw_error(sys::errc::file_exists);
            child_i->second = _flat_store(_objdir, blob);
            return true;
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
        [&] (Tree& tree) {
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

            _flat_remove(_objdir, i->second);
            i->second = _flat_store(_objdir, blob);

            return true;
        });

    return size;
}

//--------------------------------------------------------------------

size_t LocalBranch::read(PathRange path, const char* buf, size_t size, size_t offset)
{
    return BranchIo::read(_objdir, root_object_id(), path, buf, size, offset);
}

//--------------------------------------------------------------------

size_t LocalBranch::truncate(PathRange path, size_t size)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    update_dir(parent(path),
        [&] (Tree& tree) {
            auto i = tree.find(path.back().native());
            if (i == tree.end()) throw_error(sys::errc::no_such_file_or_directory);

            // XXX: Read only what's needed, not the whole blob
            auto blob = object::io::load<Blob>(_objdir, i->second);

            blob.resize(std::min<size_t>(blob.size(), size));
            size = blob.size();

            _flat_remove(_objdir, i->second);
            i->second = _flat_store(_objdir, blob);

            return true;
        });

    return size;
}

//--------------------------------------------------------------------

Opt<Blob> LocalBranch::maybe_load(const fs::path& path) const
{
    return BranchIo::maybe_load(_objdir, root_object_id(), path);
}

//--------------------------------------------------------------------

Tree LocalBranch::readdir(PathRange path) const
{
    return BranchIo::readdir(_objdir, root_object_id(), path);
}

//--------------------------------------------------------------------

FileSystemAttrib LocalBranch::get_attr(PathRange path) const
{
    return BranchIo::get_attr(_objdir, root_object_id(), path);
}

//--------------------------------------------------------------------

void LocalBranch::mkdir(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::invalid_argument);

    update_dir(parent(path),
        [&] (Tree& parent) {
            auto [i, inserted] = parent.insert(std::make_pair(path.back().native(), Id{}));
            if (!inserted) throw_error(sys::errc::file_exists);
            i->second = _flat_store(_objdir, Tree{});
            return true;
        });
}

//--------------------------------------------------------------------

bool LocalBranch::remove(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::operation_not_permitted);

    update_dir(parent(path),
        [&] (Tree& tree) {
            auto i = tree.find(path.back().native());
            if (i == tree.end()) throw_error(sys::errc::no_such_file_or_directory);
            _remove_with_children(_objdir, i->second);
            tree.erase(i);
            return true;
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
}

//--------------------------------------------------------------------

void LocalBranch::root_object_id(const object::Id& id) {
    if (_root_id == id) return;
    _root_id = id;
    _clock.increment(_user_id);
    store_self();
}

//--------------------------------------------------------------------

LocalBranch::LocalBranch(const fs::path& file_path, const fs::path& objdir,
        const UserId& user_id, Commit commit) :
    _file_path(file_path),
    _objdir(objdir),
    _user_id(user_id),
    _root_id(commit.root_object_id),
    _clock(move(commit.version_vector))
{}

LocalBranch::LocalBranch(const fs::path& file_path, const fs::path& objdir, IArchive& ar) :
    _file_path(file_path),
    _objdir(objdir)
{
    load_rest(ar);
}

//--------------------------------------------------------------------

void LocalBranch::store_tag(OArchive& ar) const
{
    ar << BranchIo::BranchType::Local;
}

void LocalBranch::store_rest(OArchive& ar) const
{
    ar << _user_id;
    ar << _root_id;
    ar << _clock;
}

void LocalBranch::load_rest(IArchive& ar)
{
    ar >> _user_id;
    ar >> _root_id;
    ar >> _clock;
}

//--------------------------------------------------------------------
