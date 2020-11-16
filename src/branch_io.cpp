#include "branch_io.h"

#include <boost/filesystem.hpp>
#include <boost/archive/binary_oarchive.hpp>
#include <boost/archive/binary_iarchive.hpp>

using namespace ouisync;

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
        return RemoteBranch{};
    }
}

