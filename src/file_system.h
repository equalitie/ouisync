#pragma once

#include "shortcuts.h"
#include "variant.h"
#include "error.h"
#include "file_system_options.h"
#include "user_id.h"
#include "branch.h"
#include "path_range.h"
#include <boost/asio/any_io_executor.hpp>
#include <boost/asio/awaitable.hpp>
#include <string>

namespace ouisync {

class FileSystem {
private:

    using Tree = boost::make_recursive_variant<
        std::string, // file data
        std::map<std::string, boost::recursive_variant_>>::type;

    using File = std::string;
    using Dir = std::map<std::string, Tree>;

public:
    using executor_type = net::any_io_executor;

public:
    struct DirAttr {};
    struct FileAttr { size_t size; };
    using Attr = variant<DirAttr, FileAttr>;

    FileSystem(executor_type ex, FileSystemOptions);

    net::awaitable<std::vector<std::string>> readdir(PathRange);
    net::awaitable<Attr> get_attr(PathRange);
    net::awaitable<size_t> read(PathRange, char* buf, size_t size, off_t offset);
    net::awaitable<int> write(PathRange, const char* buf, size_t size, off_t offset);
    net::awaitable<size_t> truncate(PathRange, size_t);
    net::awaitable<void> mknod(PathRange, mode_t mode, dev_t dev);
    net::awaitable<void> mkdir(PathRange, mode_t mode);
    net::awaitable<void> remove_file(PathRange);
    net::awaitable<void> remove_directory(PathRange);

    executor_type get_executor() { return _ex; }

private:
    Dir& find_parent(PathRange);

    Tree& find_tree(PathRange);
    template<class T> T& find(PathRange);
private:
    executor_type _ex;
    FileSystemOptions _options;
    UserId _user_id;
    std::map<UserId, Branch> _branches;
    Tree _debug_tree;
};

} // namespace
