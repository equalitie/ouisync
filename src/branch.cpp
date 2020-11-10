#include "branch.h"
#include "variant.h"
#include "error.h"
#include "path_range.h"
#include "object/tree.h"
#include "object/tagged.h"
#include "object/blob.h"
#include "object/io.h"
#include "object/refcount.h"

#include <boost/filesystem.hpp>
#include <boost/archive/text_oarchive.hpp>
#include <boost/archive/text_iarchive.hpp>
#include <boost/serialization/vector.hpp>
#include <string.h> // memcpy

#include <iostream>
#include "array_io.h"
#include "hex.h"

using namespace ouisync;
using std::move;
using object::Id;
using object::Blob;
using object::Tree;
using object::JustTag;
using Blob = Branch::Blob;
using Tree = Branch::Tree;

/* static */
Branch Branch::load_or_create(const fs::path& rootdir, const fs::path& objdir, UserId user_id) {
    object::Id root_id;
    VersionVector clock;

    fs::path path = rootdir / user_id.to_string();

    fs::fstream file(path, file.binary | file.in);

    if (!file.is_open()) {
        object::Tree root_obj;
        root_id = root_obj.store(objdir);
        object::refcount::increment(objdir, root_id);
        Branch branch(path, objdir, user_id, root_id, std::move(clock));
        branch.store_self();
        return branch;
    }

    boost::archive::text_iarchive oa(file);
    object::tagged::Load<object::Id> load{root_id};
    oa >> load;
    oa >> clock;

    return Branch{path, objdir, user_id, root_id, move(clock)};
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
void Branch::update_dir(PathRange path, F&& f)
{
    auto oid = _update_dir(_objdir, root_object_id(), path, std::forward<F>(f));

    if (oid) {
        root_object_id(*oid);
    }
}

// Same as above but make sure the tree isn't modified
template<class F>
static
void _query_dir(const fs::path& objdir, Id tree_id, PathRange path, F&& f)
{
    const Tree tree = object::io::load<Tree>(objdir, tree_id);

    if (path.empty()) {
        f(tree);
    } else {
        auto child_i = tree.find(path.front().string());
        if (child_i == tree.end()) throw_error(sys::errc::no_such_file_or_directory);
        path.advance_begin(1);
        _query_dir(objdir, child_i->second, path, std::forward<F>(f));
    }
}

template<class F>
void Branch::query_dir(PathRange path, F&& f) const
{
    _query_dir(_objdir, root_object_id(), path, std::forward<F>(f));
}

//--------------------------------------------------------------------

void Branch::store(PathRange path, const Blob& blob)
{
    auto dirpath = path;
    dirpath.advance_end(-1);

    update_dir(dirpath,
        [&] (Tree& tree) {
            auto [child_i, inserted] = tree.insert(std::make_pair(path.back().native(), Id{}));
            if (!inserted) throw_error(sys::errc::file_exists);
            child_i->second = _flat_store(_objdir, blob);
            return true;
        });
}

void Branch::store(const fs::path& path, const Blob& blob)
{
    store(path_range(path), blob);
}

//--------------------------------------------------------------------

size_t Branch::write(PathRange path, const char* buf, size_t size, size_t offset)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    auto parent_dir = path;
    parent_dir.advance_end(-1);

    update_dir(parent_dir,
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

size_t Branch::read(PathRange path, const char* buf, size_t size, size_t offset)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    auto parent_dir = path;
    parent_dir.advance_end(-1);

    query_dir(parent_dir,
        [&] (const Tree& tree) {
            auto i = tree.find(path.back().native());
            if (i == tree.end()) throw_error(sys::errc::no_such_file_or_directory);

            // XXX: Read only what's needed, not the whole blob
            auto blob = object::io::load<Blob>(_objdir, i->second);

            size_t len = blob.size();

            if (size_t(offset) < len) {
                if (offset + size > len) size = len - offset;
                memcpy((void*)buf, blob.data() + offset, size);
            } else {
                size = 0;
            }
        });

    return size;
}

//--------------------------------------------------------------------

size_t Branch::truncate(PathRange path, size_t size)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    auto parent_dir = path;
    parent_dir.advance_end(-1);

    update_dir(parent_dir,
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

Opt<Blob> Branch::maybe_load(const fs::path& path_) const
{
    PathRange path(path_);

    if (path.empty()) throw_error(sys::errc::is_a_directory);

    auto dirpath = path;
    dirpath.advance_end(-1);

    Opt<Blob> retval;

    query_dir(dirpath,
        [&] (const Tree& tree) {
            auto i = tree.find(path.back().native());
            if (i == tree.end()) throw_error(sys::errc::no_such_file_or_directory);
            retval = object::io::load<Blob>(_objdir, i->second);
        });

    return retval;
}

//--------------------------------------------------------------------

Tree Branch::readdir(PathRange path) const
{
    Opt<Tree> retval;

    query_dir(path, [&] (const Tree& tree) { retval = tree; });

    assert(retval);
    return std::move(*retval);
}

//--------------------------------------------------------------------

FileSystemAttrib Branch::get_attr(PathRange path) const
{
    if (path.empty()) return FileSystemDirAttrib{};

    auto parent_path = path;
    parent_path.advance_end(-1);

    FileSystemAttrib attrib;

    query_dir(parent_path,
        [&] (const Tree& parent) {
            auto i = parent.find(path.back().native());
            if (i == parent.end()) throw_error(sys::errc::no_such_file_or_directory);

            // XXX: Don't load the whole objects into memory.
            auto obj = object::io::load<Tree, Blob>(_objdir, i->second);

            apply(obj,
                [&] (const Tree&) { attrib = FileSystemDirAttrib{}; },
                [&] (const Blob& b) { attrib = FileSystemFileAttrib{b.size()}; });
        });

    return attrib;
}

//--------------------------------------------------------------------

void Branch::mkdir(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::invalid_argument);

    auto parent_path = path;
    parent_path.advance_end(-1);

    update_dir(parent_path,
        [&] (Tree& parent) {
            auto [i, inserted] = parent.insert(std::make_pair(path.back().native(), Id{}));
            if (!inserted) throw_error(sys::errc::file_exists);
            i->second = _flat_store(_objdir, Tree{});
            return true;
        });
}

//--------------------------------------------------------------------

bool Branch::remove(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::operation_not_permitted);

    auto parent_path = path;
    parent_path.advance_end(-1);

    update_dir(parent_path,
        [&] (Tree& tree) {
            auto i = tree.find(path.back().native());
            if (i == tree.end()) throw_error(sys::errc::no_such_file_or_directory);
            _remove_with_children(_objdir, i->second);
            tree.erase(i);
            return true;
        });

    return true;
}

bool Branch::remove(const fs::path& path)
{
    return remove(path_range(path));
}

//--------------------------------------------------------------------

void Branch::store_self() const {
    fs::fstream file(_file_path, file.binary | file.trunc | file.out);
    if (!file.is_open())
        throw std::runtime_error("Failed to open branch file");
    boost::archive::text_oarchive oa(file);
    object::tagged::Save<object::Id> save{_root_id};
    oa << save;
    oa << _clock;
}

//--------------------------------------------------------------------

void Branch::root_object_id(const object::Id& id) {
    if (_root_id == id) return;
    _root_id = id;
    _clock.increment(_user_id);
    store_self();
}

//--------------------------------------------------------------------

Branch::Branch(const fs::path& file_path, const fs::path& objdir,
        const UserId& user_id, const object::Id& root_id, VersionVector clock) :
    _file_path(file_path),
    _objdir(objdir),
    _user_id(user_id),
    _root_id(root_id),
    _clock(std::move(clock))
{}

//--------------------------------------------------------------------

