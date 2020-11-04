#include "file_system.h"

#include <iostream>
#include <boost/filesystem.hpp>
#include <boost/range/iterator_range.hpp>

using namespace ouisync;
using std::vector;
using std::map;
using std::string;
using boost::get;

namespace {
    using PathRange = boost::iterator_range<fs::path::iterator>;

    PathRange path_range(const fs::path& path) {
        return boost::make_iterator_range(path);
    }

    // Debug
    //std::ostream& operator<<(std::ostream& os, PathRange r) {
    //    fs::path path;
    //    for (auto& p : r) path /= p;
    //    return os << path;
    //}

    // Debug
    //std::string debug(const fs::path& p) noexcept {
    //    fs::ifstream t(p);
    //    return std::string((std::istreambuf_iterator<char>(t)),
    //                        std::istreambuf_iterator<char>());
    //}
}

FileSystem::FileSystem(executor_type ex) :
    _ex(std::move(ex))
{
    _debug_tree = Dir{
        { string("dir"), Dir{ { string("file"), string("content") } } },
        { string("hello"), string("world") }
    };
}

FileSystem::Tree& FileSystem::find_tree(const fs::path& path_)
{
    if (!path_.is_absolute()) throw_errno(ENOENT);
    auto path = path_range(path_);
    path.advance_begin(1);

    auto* tree = &_debug_tree;

    for (auto& p : path) {
        auto dir = get<Dir>(tree);
        assert(dir);
        if (!dir) throw_errno(ENOENT);
        auto i = dir->find(p.native());
        if (i == dir->end()) throw_errno(ENOENT);
        tree = &i->second;
    }

    return *tree;
}

template<class T>
T& FileSystem::find(const fs::path& path)
{
    auto p = get<T>(&find_tree(path));
    if (!p) throw_errno(ENOENT);
    return *p;
}

net::awaitable<FileSystem::Attr> FileSystem::get_attr(const fs::path& path)
{
    Tree& t = find_tree(path);

    co_return apply(t,
            [&] (const Dir&) -> Attr { return DirAttr{}; },
            [&] (const File& f) -> Attr { return FileAttr{f.size()}; });
}

net::awaitable<vector<string>> FileSystem::readdir(const fs::path& path)
{
    std::vector<std::string> nodes;

    for (auto& [name, val] : find<Dir>(path)) {
        (void) val;
        nodes.push_back(name);
    }

    co_return nodes;
}

net::awaitable<size_t> FileSystem::read(const fs::path& path, char* buf, size_t size, off_t offset)
{
    File& content = find<File>(path);

    size_t len = content.size();

    if (size_t(offset) < len) {
        if (offset + size > len) size = len - offset;
        memcpy(buf, content.data() + offset, size);
    } else {
        size = 0;
    }

    co_return size;

    //if(strcmp(path+1, filename) != 0) return -ENOENT;
    //size_t len = strlen(contents);
    //if (offset < len) {
    //    if (offset + size > len) size = len - offset;
    //    memcpy(buf, contents + offset, size);
    //} else {
    //    size = 0;
    //}
    //return size;
}
