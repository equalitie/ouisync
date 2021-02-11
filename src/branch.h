#pragma once

#include "object_id.h"
#include "file_blob.h"
#include "directory.h"
#include "user_id.h"
#include "version_vector.h"
#include "path_range.h"
#include "shortcuts.h"
#include "file_system_attrib.h"
#include "commit.h"
#include "branch_view.h"
#include "options.h"
#include "object_store.h"
#include "index.h"

#include <boost/filesystem/path.hpp>
#include <boost/optional.hpp>

namespace ouisync {

class Branch {
private:
    class Op;
    class TreeOp;
    class HasTreeParrentOp;
    class FileOp;
    class RootOp;
    class BranchOp;
    class RemoveOp;
    class CdOp;

    using Indices = std::map<UserId, Index>;

public:
    static
    Branch create(const fs::path& path, UserId user_id, ObjectStore&, Options::Branch);

    static
    Branch load(const fs::path& file_path, UserId user_id, ObjectStore&, Options::Branch);

    const Commit& commit() const { return _indices.find(_user_id)->second.commit(); }

    const ObjectId& root_id() const { return commit().root_id; }

    BranchView branch_view() const {
        return BranchView(_objstore, commit().root_id);
    }

    // XXX: Deprecated, use `write` instead. I believe these are currently
    // only being used in tests.
    void store(PathRange, const FileBlob&);
    void store(const fs::path&, const FileBlob&);

    size_t write(PathRange, const char* buf, size_t size, size_t offset);

    bool remove(const fs::path&);
    bool remove(PathRange);

    size_t truncate(PathRange, size_t);

    void mkdir(PathRange);

    const VersionVector& stamp() const { return commit().stamp; }

    const UserId& user_id() const { return _user_id; }

    friend std::ostream& operator<<(std::ostream&, const Branch&);

    template<class Archive>
    void serialize(Archive& ar, unsigned) {
        ar & _indices;
    }

    void sanity_check() {} // TODO

private:
    friend class BranchView;

    Branch(const fs::path& file_path, const UserId&, ObjectStore&, Options::Branch);
    Branch(const fs::path& file_path, const UserId&, Commit, ObjectStore&, Options::Branch);

    void store_self() const;

    template<class F> void update_dir(PathRange, F&&);

    std::unique_ptr<TreeOp> root();
    std::unique_ptr<TreeOp> cd_into(PathRange);
    std::unique_ptr<FileOp> get_file(PathRange);

private:
    fs::path _file_path;
    Options::Branch _options;
    ObjectStore& _objstore;
    UserId _user_id;
    Indices _indices;
};

} // namespace
