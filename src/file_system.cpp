#include "file_system.h"

#include <iostream>
#include <boost/filesystem.hpp>
#include <boost/range/iterator_range.hpp>

using namespace ouisync;
using std::vector;
using std::map;
using std::string;
using boost::get;

FileSystem::FileSystem(executor_type ex, FileSystemOptions options) :
    _ex(std::move(ex)),
    _options(std::move(options))
{
    _debug_tree = Dir{
        { string("dir"), Dir{ { string("file"), string("content") } } },
        { string("hello"), string("world") }
    };

    _user_id = UserId::load_or_create(_options.user_id_file_path());

    auto branch = Branch::load_or_create(
            _options.branchdir(),
            _options.objectdir(),
            _user_id);

    _branches.insert(std::make_pair(_user_id, std::move(branch)));
}

FileSystem::Tree& FileSystem::find_tree(PathRange path)
{
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
T& FileSystem::find(PathRange path_range)
{
    auto p = get<T>(&find_tree(path_range));
    if (!p) throw_errno(EINVAL);
    return *p;
}

FileSystem::Dir& FileSystem::find_parent(PathRange path)
{
    if (path.begin() == path.end()) throw_errno(EINVAL);
    path.advance_end(-1);
    return find<Dir>(path);
}

net::awaitable<FileSystem::Attr> FileSystem::get_attr(PathRange path)
{

    Tree& t = find_tree(path);

    co_return apply(t,
            [&] (const Dir&) -> Attr { return DirAttr{}; },
            [&] (const File& f) -> Attr { return FileAttr{f.size()}; });
}

net::awaitable<vector<string>> FileSystem::readdir(PathRange path)
{
    std::vector<std::string> nodes;

    for (auto& [name, val] : find<Dir>(path)) {
        (void) val;
        nodes.push_back(name);
    }

    co_return nodes;
}

net::awaitable<size_t> FileSystem::read(PathRange path, char* buf, size_t size, off_t offset)
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

net::awaitable<int> FileSystem::write(PathRange path, const char* buf, size_t size, off_t offset)
{
    File& content = find<File>(path);

    size_t len = content.size();

    if (offset + size > len) {
        content.resize(offset + size);
    }

    memcpy(content.data() + offset, buf, size);

    co_return size;
}

net::awaitable<void> FileSystem::mknod(PathRange path, mode_t mode, dev_t dev)
{
    if (S_ISFIFO(mode)) throw_errno(EINVAL); // TODO?
    Dir& dir = find_parent(path);
    auto inserted = dir.insert({path.back().native(), File{}}).second;
    if (!inserted) throw_errno(EEXIST);
    co_return;
}

net::awaitable<void> FileSystem::mkdir(PathRange path, mode_t mode)
{
    Dir& dir = find_parent(path);
    // Why does path.back() cause an asan crash?
    auto inserted = dir.insert({(--path.end())->native(), Dir{}}).second;
    if (!inserted) throw_errno(EEXIST);
    co_return;
}

net::awaitable<void> FileSystem::remove_file(PathRange path)
{
    Dir& dir = find_parent(path);
    size_t n = dir.erase(path.back().native());
    if (n == 0) throw_errno(EEXIST);
    co_return;
}

net::awaitable<void> FileSystem::remove_directory(PathRange path)
{
    Dir& dir = find_parent(path);
    size_t n = dir.erase(path.back().native());
    if (n == 0) throw_errno(EEXIST);
    co_return;
}

net::awaitable<size_t> FileSystem::truncate(PathRange path, size_t size)
{
    File& content = find<File>(path);
    content.resize(size);
    co_return content.size();
}
