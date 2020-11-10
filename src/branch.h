#pragma once

#include "object/id.h"
#include "object/blob.h"
#include "object/tree.h"
#include "user_id.h"
#include "version_vector.h"
#include "path_range.h"
#include "shortcuts.h"
#include "file_system_attrib.h"

#include <boost/filesystem/path.hpp>
#include <boost/optional.hpp>

namespace ouisync {

class Branch {
public:
    using Blob = object::Blob;
    using Tree = object::Tree;

public:
    static Branch load_or_create(
            const fs::path& branchdir, const fs::path& objdir, UserId);

    const object::Id& root_object_id() const {
        return _root_id;
    }

    void root_object_id(const object::Id& id);

    bool maybe_store(const fs::path&, const Blob&);

    void store(PathRange, const Blob&);
    void store(const fs::path&, const Blob&);

    Opt<Blob> maybe_load(const fs::path&) const;

    Tree readdir(PathRange) const;
    FileSystemAttrib get_attr(PathRange) const;

    bool remove(const fs::path&);

    const fs::path& object_directory() const {
        return _objdir;
    }

    void mkdir(PathRange);
    void rmdir(PathRange);

private:
    Branch(const fs::path& file_path, const fs::path& objdir,
            const UserId& user_id, const object::Id& root_id,
            VersionVector clock);

    void store_self() const;

private:
    fs::path _file_path;
    fs::path _objdir;
    UserId _user_id;
    object::Id _root_id;
    VersionVector _clock;
};

} // namespace
