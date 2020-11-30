#pragma once

#include "object_id.h"
#include "object/blob.h"
#include "object/tree.h"
#include "user_id.h"
#include "version_vector.h"
#include "path_range.h"
#include "shortcuts.h"
#include "file_system_attrib.h"
#include "commit.h"
#include "branch_io.h"

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

    LocalBranch(const fs::path& file_path, const fs::path& objdir, IArchive&);

    LocalBranch(const fs::path& file_path, const fs::path& objdir,
            const UserId&, Commit);

    const ObjectId& root_id() const { return _root_id; }

    BranchIo::Immutable immutable_io() const {
        return BranchIo::Immutable(_objdir, _root_id);
    }

    // XXX: Deprecated, use `write` instead. I believe these are currently
    // only being used in tests.
    void store(PathRange, const Blob&);
    void store(const fs::path&, const Blob&);

    size_t write(PathRange, const char* buf, size_t size, size_t offset);

    bool remove(const fs::path&);
    bool remove(PathRange);

    size_t truncate(PathRange, size_t);

    const fs::path& object_directory() const {
        return _objdir;
    }

    void mkdir(PathRange);

    const VersionVector& stamp() const { return _stamp; }

    const UserId& user_id() const { return _user_id; }

    ObjectId id_of(PathRange) const;

    bool introduce_commit(const Commit&);

    friend std::ostream& operator<<(std::ostream&, const LocalBranch&);

private:
    friend class BranchIo;

    void set_root_id(const ObjectId& id);

    void store_tag(OArchive&) const;
    void store_rest(OArchive&) const;
    void load_rest(IArchive&);

    void store_self() const;

    template<class F> void update_dir(PathRange, F&&);

private:
    fs::path _file_path;
    fs::path _objdir;
    UserId _user_id;
    ObjectId _root_id;
    VersionVector _stamp;
};

} // namespace
