#include "branch_io.h"
#include "error.h"
#include "refcount.h"
#include "object/io.h"
#include "object/tagged.h"

#include <boost/filesystem.hpp>
#include <boost/serialization/vector.hpp>

using namespace ouisync;
using object::Tree;
using object::Blob;

//--------------------------------------------------------------------

template<class F>
static
void _query_dir(const fs::path& objdir, ObjectId tree_id, PathRange path, F&& f)
{
    const Tree tree = object::io::load<Tree>(objdir, tree_id);

    if (path.empty()) {
        f(tree);
    } else {
        auto child = tree.find(path.front());
        if (!child) throw_error(sys::errc::no_such_file_or_directory);
        path.advance_begin(1);
        _query_dir(objdir, child.id(), path, std::forward<F>(f));
    }
}

static
PathRange _parent(PathRange path) {
    path.advance_end(-1);
    return path;
}

//--------------------------------------------------------------------

BranchIo::Immutable::Immutable(const fs::path& objdir, const ObjectId& root_id) :
    _objdir(objdir),
    _root_id(root_id)
{}

//--------------------------------------------------------------------

Tree BranchIo::Immutable::readdir(PathRange path) const
{
    Opt<Tree> retval;
    _query_dir(_objdir, _root_id, path, [&] (const Tree& tree) { retval = tree; });
    assert(retval);
    return move(*retval);
}

//--------------------------------------------------------------------

FileSystemAttrib BranchIo::Immutable::get_attr(PathRange path) const
{
    if (path.empty()) return FileSystemDirAttrib{};

    FileSystemAttrib attrib;

    _query_dir(_objdir, _root_id, _parent(path),
        [&] (const Tree& parent) {
            auto child = parent.find(path.back());
            if (!child) throw_error(sys::errc::no_such_file_or_directory);

            auto obj = object::io::load<Tree::Nothing, Blob::Size>(_objdir, child.id());

            apply(obj,
                [&] (const Tree::Nothing&) { attrib = FileSystemDirAttrib{}; },
                [&] (const Blob::Size& b) { attrib = FileSystemFileAttrib{b.value}; });
        });

    return attrib;
}

//--------------------------------------------------------------------

size_t BranchIo::Immutable::read(PathRange path, const char* buf, size_t size, size_t offset) const
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    _query_dir(_objdir, _root_id, _parent(path),
        [&] (const Tree& tree) {
            auto child = tree.find(path.back());
            if (!child) throw_error(sys::errc::no_such_file_or_directory);

            // XXX: Read only what's needed, not the whole blob
            auto blob = object::io::load<Blob>(_objdir, child.id());

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

Opt<Blob> BranchIo::Immutable::maybe_load(PathRange path) const
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    Opt<Blob> retval;

    _query_dir(_objdir, _root_id, _parent(path),
        [&] (const Tree& tree) {
            auto child = tree.find(path.back());
            if (!child) throw_error(sys::errc::no_such_file_or_directory);
            retval = object::io::load<Blob>(_objdir, child.id());
        });

    return retval;
}

//--------------------------------------------------------------------

ObjectId BranchIo::Immutable::id_of(PathRange path) const
{
    if (path.empty()) return _root_id;

    ObjectId retval;

    _query_dir(_objdir, _root_id, _parent(path),
        [&] (const Tree& tree) {
            auto child = tree.find(path.back());
            if (!child) throw_error(sys::errc::no_such_file_or_directory);
            retval = child.id();
        });

    return retval;
}

//--------------------------------------------------------------------

static
void _show(std::ostream& os, fs::path objdir, ObjectId id, std::string pad = "") {
    if (!object::io::exists(objdir, id)) {
        os << pad << "!!! object " << id << " does not exist !!!\n";
        return;
    }

    auto obj = object::io::load<Tree, Blob>(objdir, id);
    auto rc = Rc::load(objdir, id);

    apply(obj,
            [&] (const Tree& t) {
                os << pad << t << " (" << rc << ")\n";
                for (auto& [name, id] : t) {
                    _show(os, objdir, id, pad + "  ");
                }
            },
            [&] (const Blob& b) {
                os << pad << b << " (" << rc << ")\n";
            });
}

void BranchIo::Immutable::show(std::ostream& os) const
{
    return _show(os, _objdir, _root_id, "");
}

//--------------------------------------------------------------------

bool BranchIo::Immutable::object_exists(const ObjectId& id) const
{
    return object::io::exists(_objdir, id);
}

//--------------------------------------------------------------------
