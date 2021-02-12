#pragma once

#include "shortcuts.h"
#include "options.h"
#include "file_system_attrib.h"
#include "user_id.h"
#include "branch.h"
#include "commit.h"
#include "object_store.h"

#include <boost/asio/any_io_executor.hpp>
#include <boost/asio/awaitable.hpp>
#include <string>

namespace ouisync {

class Repository {
public:
    using executor_type = net::any_io_executor;

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

    const fs::path& object_directory() const { return _options.objectdir; }

    ObjectStore& object_store() { return _objects; }

    Branch& branch() { return *_branch; }

private:
    void sanity_check() const;

private:
    executor_type _ex;
    const Options _options;
    ObjectStore _objects;
    UserId _user_id;
    std::unique_ptr<Branch> _branch;
};

} // namespace
