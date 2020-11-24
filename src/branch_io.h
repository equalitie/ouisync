#pragma once

#include "object/tree.h"
#include "object/blob.h"
#include "path_range.h"
#include "file_system_attrib.h"

#include <boost/filesystem/path.hpp>

namespace ouisync {

class BranchIo {
public:
    using Id = object::Id;
    using Tree = object::Tree;
    using Blob = object::Blob;

public:
    class Immutable {
    public:
        Immutable(const fs::path& objdir, const Id& root_id);

        Tree readdir(PathRange path) const;

        FileSystemAttrib get_attr(PathRange path) const;

        size_t read(PathRange path, const char* buf, size_t size, size_t offset) const;

        Opt<Blob> maybe_load(PathRange) const;

        Id id_of(PathRange) const;

        void show(std::ostream&) const;

    private:
        const fs::path& _objdir;
        const Id& _root_id;
    };
};

} // namespace
