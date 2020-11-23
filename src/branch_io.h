#pragma once

#include "object/tree.h"
#include "local_branch.h"
#include "remote_branch.h"

#include <boost/filesystem/path.hpp>

namespace ouisync {

class BranchIo {
public:
    enum class BranchType { Local, Remote };
    using Id = object::Id;
    using Branch = variant<LocalBranch, RemoteBranch>;
    using Tree = object::Tree;
    using Blob = object::Blob;

public:
    static
    Branch load(const fs::path& path, const fs::path& objdir);

    static
    Tree readdir(const fs::path& objdir, Id root_id, PathRange path);

    static
    FileSystemAttrib get_attr(const fs::path& objdir, Id root_id, PathRange path);

    static
    size_t read(const fs::path& objdir, Id root_id, PathRange path,
        const char* buf, size_t size, size_t offset);

    static
    Opt<Blob> maybe_load(const fs::path& objdir, Id root_id, PathRange);

    static
    Id id_of(const fs::path& objdir, const Id& root_id, PathRange);

    static
    void show(std::ostream&, const fs::path& objdir, const Id& root_id);
};

} // namespace
