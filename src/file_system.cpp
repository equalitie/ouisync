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
        if (!dir) throw_error(sys::errc::no_such_file_or_directory);
        auto i = dir->find(p.native());
        if (i == dir->end()) throw_error(sys::errc::no_such_file_or_directory);
        tree = &i->second;
    }

    return *tree;
}

template<class T>
T& FileSystem::find(PathRange path_range)
{
    auto p = get<T>(&find_tree(path_range));
    if (!p) throw_error(sys::errc::invalid_argument);
    return *p;
}

FileSystem::Dir& FileSystem::find_parent(PathRange path)
{
    if (path.begin() == path.end()) throw_error(sys::errc::invalid_argument);
    path.advance_end(-1);
    return find<Dir>(path);
}

Branch& FileSystem::find_branch(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::invalid_argument);
    auto user_id = UserId::from_string(path.front().native());
    if (!user_id) throw_error(sys::errc::invalid_argument);
    auto i = _branches.find(*user_id);
    if (i == _branches.end()) throw_error(sys::errc::invalid_argument);
    return i->second;
}

net::awaitable<FileSystem::Attrib> FileSystem::get_attr(PathRange path)
{
    if (path.empty()) co_return DirAttrib{};

    auto& branch = find_branch(path);

    path.advance_begin(1);
    auto ret = branch.get_attr(path);
    co_return ret;
}

net::awaitable<vector<string>> FileSystem::readdir(PathRange path)
{
    std::vector<std::string> nodes;

    if (path.empty()) {
        for (auto& [name, branch] : _branches) {
            (void) branch;
            nodes.push_back(name.to_string());
        }
    }
    else {
        auto& branch = find_branch(path);

        path.advance_begin(1);
        auto dir = branch.readdir(path);

        for (auto& [name, hash] : dir) {
            nodes.push_back(name);
        }
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
    if (S_ISFIFO(mode)) throw_error(sys::errc::invalid_argument); // TODO?
    Dir& dir = find_parent(path);
    auto inserted = dir.insert({path.back().native(), File{}}).second;
    if (!inserted) throw_error(sys::errc::file_exists);
    co_return;
}

net::awaitable<void> FileSystem::mkdir(PathRange path, mode_t mode)
{
    if (path.empty()) {
        // The root directory is reserved for branches, users can't create
        // new directories there.
        throw_error(sys::errc::operation_not_permitted);
    }

    auto& branch = find_branch(path);
    path.advance_begin(1);
    branch.mkdir(path);
    co_return;
}

net::awaitable<void> FileSystem::remove_file(PathRange path)
{
    Dir& dir = find_parent(path);
    size_t n = dir.erase(path.back().native());
    if (n == 0) throw_error(sys::errc::file_exists);
    co_return;
}

net::awaitable<void> FileSystem::remove_directory(PathRange path)
{
    Dir& dir = find_parent(path);
    size_t n = dir.erase(path.back().native());
    if (n == 0) throw_error(sys::errc::file_exists);
    co_return;
}

net::awaitable<size_t> FileSystem::truncate(PathRange path, size_t size)
{
    File& content = find<File>(path);
    content.resize(size);
    co_return content.size();
}
