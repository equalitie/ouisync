#pragma once

#include "object/id.h"
#include "object/blob.h"
#include "object/tree.h"
#include "user_id.h"
#include "version_vector.h"
#include "path_range.h"
#include "shortcuts.h"
#include "file_system_attrib.h"
#include "commit.h"

#include <boost/filesystem/path.hpp>
#include <boost/optional.hpp>

namespace boost::archive {
    class binary_iarchive;
    class binary_oarchive;
} // boost::archive namespace

namespace ouisync {

class LocalBranch {
private:
    using IArchive = boost::archive::binary_iarchive;
    using OArchive = boost::archive::binary_oarchive;

public:
    using Blob = object::Blob;
    using Tree = object::Tree;

public:
    static
    LocalBranch create(const fs::path& path, const fs::path& objdir, UserId user_id);

    const object::Id& root_object_id() const {
        return _root_id;
    }

    void root_object_id(const object::Id& id);

    bool maybe_store(const fs::path&, const Blob&);

    // XXX: Deprecated, use `write` instead. I believe these are currently
    // only being used in tests.
    void store(PathRange, const Blob&);
    void store(const fs::path&, const Blob&);

    size_t write(PathRange, const char* buf, size_t size, size_t offset);
    size_t read(PathRange, const char* buf, size_t size, size_t offset);

    // XXX: Deprecated, use `read` instead. I believe these are currently
    // only being used in tests.
    Opt<Blob> maybe_load(const fs::path&) const;

    Tree readdir(PathRange) const;
    FileSystemAttrib get_attr(PathRange) const;

    bool remove(const fs::path&);
    bool remove(PathRange);

    size_t truncate(PathRange, size_t);

    const fs::path& object_directory() const {
        return _objdir;
    }

    void mkdir(PathRange);

    const VersionVector& version_vector() const { return _clock; }

    const UserId& user_id() const { return _user_id; }

private:
    friend class BranchIo;

    void store_tag(OArchive&) const;
    void store_rest(OArchive&) const;
    void load_rest(IArchive&);

    LocalBranch(const fs::path& file_path, const fs::path& objdir, IArchive&);

    LocalBranch(const fs::path& file_path, const fs::path& objdir,
            const UserId&, Commit);

    void store_self() const;

    template<class F> void update_dir(PathRange, F&&);

private:
    fs::path _file_path;
    fs::path _objdir;
    UserId _user_id;
    object::Id _root_id;
    VersionVector _clock;
};

} // namespace
