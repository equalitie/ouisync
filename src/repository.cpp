#include "repository.h"
#include "random.h"
#include "hex.h"
#include "archive.h"
#include "defer.h"
#include "error.h"

#include <iostream>
#include <boost/filesystem.hpp>
#include <boost/range/iterator_range.hpp>

#include <boost/serialization/vector.hpp>

#define SANITY_CHECK_EACH_IO_FN false
#define DEBUG_PRINT_CALL false

using namespace ouisync;
using std::vector;
using std::map;
using std::string;
using boost::get;
using std::make_pair;
using std::cerr;

Repository::Repository(executor_type ex, Options options) :
    _ex(std::move(ex)),
    _options(std::move(options)),
    _objects(_options.objectdir),
    _block_store(_objects)
{
    _user_id = UserId::load_or_generate_random(_options.user_id_file_path);

    fs::path branch_path = _options.basedir / "branch";

    if (fs::exists(branch_path)) {
        _branch.reset(new Branch(Branch::load(_ex, branch_path, _user_id, _objects, _block_store, _options)));
    } else {
        _branch.reset(new Branch(Branch::create(_ex, branch_path, _user_id, _objects, _block_store, _options)));
    }

    std::cout << "User ID: " << _user_id << "\n";
}

net::awaitable<Repository::Attrib> Repository::get_attr(PathRange path) try
{
#if DEBUG_PRINT_CALL
    cerr << "Enter: Repository::get_attr " << path << "\n";
    auto at_exit = defer([&] { cerr << "Leave: Repository::get_attr\n"; });
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto check_at_exit = defer([&] { sanity_check(); });
#endif

    co_return _branch->get_attr(path);
}
catch (const std::exception& e) {
#if DEBUG_PRINT_CALL
    cerr << "Exception: Repository::get_attr " << path << " what: " << e.what() << "\n";
#endif
    throw;
}

net::awaitable<vector<string>> Repository::readdir(PathRange path) try
{
#if DEBUG_PRINT_CALL
    cerr << "Enter: Repository::readdir " << path << "\n";
    auto at_exit = defer([&] { cerr << "Leave: Repository::readdir\n"; });
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto check_at_exit = defer([&] { sanity_check(); });
#endif

    std::vector<std::string> nodes;

    auto dir = _branch->readdir(path);

    for (auto& name : dir) {
        nodes.push_back(name);
    }

    co_return nodes;
}
catch (const std::exception& e) {
#if DEBUG_PRINT_CALL
    cerr << "Exception: Repository::readdir " << path << " what: " << e.what() << "\n";
#endif
    throw;
}

net::awaitable<size_t> Repository::read(PathRange path, char* buf, size_t size, off_t offset) try
{
#if DEBUG_PRINT_CALL
    cerr << "Enter: Repository::read " << path << "\n";
    auto at_exit = defer([&] { cerr << "Leave: Repository::read\n"; });
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto check_at_exit = defer([&] { sanity_check(); });
#endif

    if (path.empty()) {
        throw_error(sys::errc::is_a_directory);
    }

    co_return _branch->read(path, buf, size, offset);
}
catch (const std::exception& e) {
#if DEBUG_PRINT_CALL
    cerr << "Exception: Repository::read " << path << " what: " << e.what() << "\n";
#endif
    throw;
}

net::awaitable<size_t> Repository::write(PathRange path, const char* buf, size_t size, off_t offset) try
{
#if DEBUG_PRINT_CALL
    cerr << "Enter: Repository::write " << path << "\n";
    auto at_exit = defer([&] { cerr << "Leave: Repository::write\n"; });
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto check_at_exit = defer([&] { sanity_check(); });
#endif

    if (path.empty()) {
        throw_error(sys::errc::is_a_directory);
    }

    auto retval = _branch->write(path, buf, size, offset);

    co_return retval;
}
catch (const std::exception& e) {
#if DEBUG_PRINT_CALL
    cerr << "Exception: Repository::write " << path << " what: " << e.what() << "\n";
#endif
    throw;
}

net::awaitable<void> Repository::mknod(PathRange path, mode_t mode, dev_t dev) try
{
#if DEBUG_PRINT_CALL
    cerr << "Enter: Repository::mknod " << path << "\n";
    auto at_exit = defer([&] { cerr << "Leave: Repository::mknod\n"; });
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto check_at_exit = defer([&] { sanity_check(); });
#endif

    if (S_ISFIFO(mode)) throw_error(sys::errc::invalid_argument); // TODO?

    _branch->mknod(path);

    co_return;
}
catch (const std::exception& e) {
#if DEBUG_PRINT_CALL
    cerr << "Exception: Repository::mknod " << path << " what: " << e.what() << "\n";
#endif
    throw;
}

net::awaitable<void> Repository::mkdir(PathRange path, mode_t mode) try
{
#if DEBUG_PRINT_CALL
    cerr << "Enter: Repository::mkdir " << path << "\n";
    auto at_exit = defer([&] { cerr << "Leave: Repository::mkdir\n"; });
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto check_at_exit = defer([&] { sanity_check(); });
#endif

    _branch->mkdir(path);

    co_return;
}
catch (const std::exception& e) {
#if DEBUG_PRINT_CALL
    cerr << "Exception: Repository::mkdir " << path << " what: " << e.what() << "\n";
#endif
    throw;
}

net::awaitable<void> Repository::remove_file(PathRange path) try
{
#if DEBUG_PRINT_CALL
    cerr << "Enter: Repository::remove_file " << path << "\n";
    auto at_exit = defer([&] { cerr << "Leave: Repository::remove_file\n"; });
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto check_at_exit = defer([&] { sanity_check(); });
#endif

    if (path.empty()) {
        throw_error(sys::errc::operation_not_permitted);
    }

    _branch->remove(path);

    co_return;
}
catch (const std::exception& e) {
#if DEBUG_PRINT_CALL
    cerr << "Exception: Repository::remove_file " << path << " what: " << e.what() << "\n";
#endif
    throw;
}

net::awaitable<void> Repository::remove_directory(PathRange path) try
{
#if DEBUG_PRINT_CALL
    cerr << "Enter: Repository::remove_directory " << path << "\n";
    auto at_exit = defer([&] { cerr << "Leave: Repository::remove_directory\n"; });
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto check_at_exit = defer([&] { sanity_check(); });
#endif

    if (path.empty()) {
        throw_error(sys::errc::operation_not_permitted);
    }

    _branch->remove(path);

    co_return;
}
catch (const std::exception& e) {
#if DEBUG_PRINT_CALL
    cerr << "Exception: Repository::remove_directory " << path << " what: " << e.what() << "\n";
#endif
    throw;
}

net::awaitable<size_t> Repository::truncate(PathRange path, size_t size) try
{
#if DEBUG_PRINT_CALL
    cerr << "Enter: Repository::truncate " << path << "\n";
    auto at_exit = defer([&] { cerr << "Leave: Repository::truncate\n"; });
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto check_at_exit = defer([&] { sanity_check(); });
#endif

    if (path.empty()) {
        throw_error(sys::errc::is_a_directory);
    }

    auto retval = _branch->truncate(path, size);

    co_return retval;
}
catch (const std::exception& e) {
#if DEBUG_PRINT_CALL
    cerr << "Exception: Repository::truncate " << path << " what: " << e.what() << "\n";
#endif
    throw;
}

void Repository::sanity_check() const
{
    _branch->sanity_check();
}
