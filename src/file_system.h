#pragma once

#include "shortcuts.h"
#include "variant.h"
#include "error.h"
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

    FileSystem(executor_type ex);

    net::awaitable<std::vector<std::string>> readdir(const fs::path&);
    net::awaitable<Attr> get_attr(const fs::path&);
    net::awaitable<size_t> read(const fs::path&, char* buf, size_t size, off_t offset);

    executor_type get_executor() { return _ex; }

private:
    template<class T> T& find(const fs::path& path);
    Tree& find_tree(const fs::path& path);

private:
    executor_type _ex;
    Tree _debug_tree;
};

} // namespace
