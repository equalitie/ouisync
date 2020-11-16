#pragma once

#include "local_branch.h"
#include "remote_branch.h"

#include <boost/filesystem/path.hpp>

namespace ouisync {

class BranchIo {
public:
    enum class BranchType { Local, Remote };

public:
    using Branch = variant<LocalBranch, RemoteBranch>;

    static
    Branch load(const fs::path& path, const fs::path& objdir);
};

} // namespace
