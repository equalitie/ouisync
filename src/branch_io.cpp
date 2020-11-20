#include "branch_io.h"
#include "error.h"
#include "object/refcount.h"
#include "object/io.h"
#include "object/tagged.h"

#include <boost/filesystem.hpp>
#include <boost/archive/binary_oarchive.hpp>
#include <boost/archive/binary_iarchive.hpp>
#include <boost/serialization/vector.hpp>

using namespace ouisync;
using object::Id;
using object::Tree;
using object::Blob;
using object::JustTag;

/* static */
BranchIo::Branch BranchIo::load(const fs::path& path, const fs::path& objdir)
{
    fs::fstream file(path, file.binary | file.in);
    boost::archive::binary_iarchive archive(file);

    BranchType type;

    archive >> type;

    assert(type == BranchType::Local || type == BranchType::Remote);

    if (type == BranchType::Local) {
        return LocalBranch(path, objdir, archive);
    } else {
        return RemoteBranch(path, objdir, archive);
    }
}

//--------------------------------------------------------------------

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

static
PathRange _parent(PathRange path) {
    path.advance_end(-1);
    return path;
}

/* static */
Tree BranchIo::readdir(const fs::path& objdir, Id root_id, PathRange path)
{
    Opt<Tree> retval;
    _query_dir(objdir, root_id, path, [&] (const Tree& tree) { retval = tree; });
    assert(retval);
    return move(*retval);
}

/* static */
FileSystemAttrib BranchIo::get_attr(const fs::path& objdir, Id root_id, PathRange path)
{
    if (path.empty()) return FileSystemDirAttrib{};

    FileSystemAttrib attrib;

    _query_dir(objdir, root_id, _parent(path),
        [&] (const Tree& parent) {
            auto i = parent.find(path.back().native());
            if (i == parent.end()) throw_error(sys::errc::no_such_file_or_directory);

            // XXX: Don't load the whole objects into memory.
            auto obj = object::io::load<Tree, Blob>(objdir, i->second);

            apply(obj,
                [&] (const Tree&) { attrib = FileSystemDirAttrib{}; },
                [&] (const Blob& b) { attrib = FileSystemFileAttrib{b.size()}; });
        });

    return attrib;
}

/* static */
size_t BranchIo::read(const fs::path& objdir, Id root_id, PathRange path,
        const char* buf, size_t size, size_t offset)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    _query_dir(objdir, root_id, _parent(path),
        [&] (const Tree& tree) {
            auto i = tree.find(path.back().native());
            if (i == tree.end()) throw_error(sys::errc::no_such_file_or_directory);

            // XXX: Read only what's needed, not the whole blob
            auto blob = object::io::load<Blob>(objdir, i->second);

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

/* static */
Opt<Blob> BranchIo::maybe_load(const fs::path& objdir, Id root_id, PathRange path)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    Opt<Blob> retval;

    _query_dir(objdir, root_id, _parent(path),
        [&] (const Tree& tree) {
            auto i = tree.find(path.back().native());
            if (i == tree.end()) throw_error(sys::errc::no_such_file_or_directory);
            retval = object::io::load<Blob>(objdir, i->second);
        });

    return retval;
}

/* static */
Id BranchIo::id_of(const fs::path& objdir, const Id& root_id, PathRange path)
{
    if (path.empty()) return root_id;

    Id retval;

    _query_dir(objdir, root_id, _parent(path),
        [&] (const Tree& tree) {
            auto i = tree.find(path.back().native());
            if (i == tree.end()) throw_error(sys::errc::no_such_file_or_directory);
            retval = i->second;
        });

    return retval;
}
