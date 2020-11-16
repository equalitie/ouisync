#include "remote_branch.h"
#include "branch_io.h"

using namespace ouisync;

using std::move;
using object::Id;
using object::Tree;

Tree RemoteBranch::readdir(PathRange path) const
{
    return BranchIo::readdir(_objdir, _root, path);
}

FileSystemAttrib RemoteBranch::get_attr(PathRange path) const
{
    return BranchIo::get_attr(_objdir, _root, path);
}

size_t RemoteBranch::read(PathRange path, const char* buf, size_t size, size_t offset) const
{
    return BranchIo::read(_objdir, _root, path, buf, size, offset);
}
