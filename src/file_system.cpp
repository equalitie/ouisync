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
    inline
    std::ostream& __attribute__((unused)) operator<<(std::ostream& os, PathRange r) {
        fs::path path;
        for (auto& p : r) path /= p;
        return os << path;
    }
}

FileSystem::FileSystem(executor_type ex) :
    _ex(std::move(ex))
{
    _debug_tree = Dir{
        { string("dir"), Dir{ { string("file"), string("content") } } },
        { string("hello"), string("world") }
    };
}

template<class PathRange>
FileSystem::Tree& FileSystem::find_tree(PathRange path)
{
    if (path.begin() == path.end()) throw_errno(EINVAL);

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

template<class T, class PathRange>
T& FileSystem::find(PathRange path_range)
{
    auto p = get<T>(&find_tree(path_range));
    if (!p) throw_errno(EINVAL);
    return *p;
}

FileSystem::Tree& FileSystem::find_tree(const fs::path& path) {
    return find_tree(path_range(path));
}

template<class T>
T& FileSystem::find(const fs::path& path) {
    return find<T>(path_range(path));
}

FileSystem::Dir& FileSystem::find_parent(const fs::path& path_)
{
    auto path = path_range(path_);
    if (path.begin() == path.end()) throw_errno(EINVAL);

    auto dirpath = path;
    dirpath.advance_end(-1);
    return find<Dir>(dirpath);
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
}

net::awaitable<void> FileSystem::mknod(const fs::path& path, mode_t mode, dev_t dev)
{
    if (S_ISFIFO(mode)) throw_errno(EINVAL); // TODO?
    Dir& dir = find_parent(path);
    auto pr = path_range(path);
    auto inserted = dir.insert({pr.back().native(), File{}}).second;
    if (!inserted) throw_errno(EEXIST);
    co_return;
}

net::awaitable<void> FileSystem::mkdir(const fs::path& path, mode_t mode)
{
    Dir& dir = find_parent(path);
    auto pr = path_range(path);
    // Why does pr.back() cause an asan crash?
    auto inserted = dir.insert({(--pr.end())->native(), Dir{}}).second;
    if (!inserted) throw_errno(EEXIST);
    co_return;
}

net::awaitable<void> FileSystem::remove_file(const fs::path& path)
{
    Dir& dir = find_parent(path);
    auto pr = path_range(path);
    size_t n = dir.erase(pr.back().native());
    if (n == 0) throw_errno(EEXIST);
    co_return;
}

net::awaitable<void> FileSystem::remove_directory(const fs::path& path)
{
    Dir& dir = find_parent(path);
    auto pr = path_range(path);
    size_t n = dir.erase(pr.back().native());
    if (n == 0) throw_errno(EEXIST);
    co_return;
}

net::awaitable<size_t> FileSystem::truncate(const fs::path& path, size_t size)
{
    File& content = find<File>(path);
    content.resize(size);
    co_return content.size();
}
