#pragma once

#include "shortcuts.h"
#include "error.h"
#include "options.h"
#include "file_system_attrib.h"
#include "user_id.h"
#include "local_branch.h"
#include "remote_branch.h"
#include "branch_io.h"
#include "commit.h"
#include "wait.h"
#include "snapshot.h"

#include <boost/asio/any_io_executor.hpp>
#include <boost/asio/awaitable.hpp>
#include <string>

namespace ouisync {

class Repository {
public:
    using executor_type = net::any_io_executor;
    using Branch = variant<LocalBranch, RemoteBranch>;

public:
    using DirAttrib  = FileSystemDirAttrib;
    using FileAttrib = FileSystemFileAttrib;
    using Attrib     = FileSystemAttrib;

    Repository(executor_type ex, Options);

    net::awaitable<std::vector<std::string>> readdir(PathRange);
    net::awaitable<Attrib> get_attr(PathRange);
    net::awaitable<size_t> read(PathRange, char* buf, size_t size, off_t offset);
    net::awaitable<size_t> write(PathRange, const char* buf, size_t size, off_t offset);
    net::awaitable<size_t> truncate(PathRange, size_t);
    net::awaitable<void> mknod(PathRange, mode_t mode, dev_t dev);
    net::awaitable<void> mkdir(PathRange, mode_t mode);
    net::awaitable<void> remove_file(PathRange);
    net::awaitable<void> remove_directory(PathRange);

    executor_type get_executor() { return _ex; }

    SnapshotGroup create_snapshot_group();

    // Note: may return nullptr if the version vector is below a version vector
    // of an already existing branch.
    [[nodiscard]]
    net::awaitable<RemoteBranch*>
    get_or_create_remote_branch(const UserId&, const Commit&);

    Opt<Snapshot::Id> last_snapshot_id() const { return _last_snapshot_id; }

    Wait& on_change() { return _on_change; }

    const fs::path& object_directory() const { return _options.objectdir; }

private:
    Branch& find_branch(PathRange);

    static
    const VersionVector& get_stamp(const Branch& b);

    static
    ObjectId get_root_id(const Branch& b);

private:
    executor_type _ex;
    const Options _options;
    UserId _user_id;
    std::map<UserId, Branch> _branches;
    Opt<Snapshot::Id> _last_snapshot_id;
    Wait _on_change;
};

} // namespace
