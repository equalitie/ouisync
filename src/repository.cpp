#include "repository.h"
#include "branch_view.h"
#include "random.h"
#include "hex.h"
#include "archive.h"
#include "defer.h"
#include "error.h"

#include <iostream>
#include <boost/filesystem.hpp>
#include <boost/range/iterator_range.hpp>

#include <boost/serialization/vector.hpp>

using namespace ouisync;
using std::vector;
using std::map;
using std::string;
using boost::get;
using std::make_pair;

#define SANITY_CHECK_EACH_IO_FN false
#define DEBUG_PRINT_CALL false

Repository::Repository(executor_type ex, Options options) :
    _ex(std::move(ex)),
    _options(std::move(options)),
    _objects(_options.objectdir),
    _on_change(_ex)
{
    _user_id = UserId::load_or_generate_random(_options.user_id_file_path);

    fs::path branch_path = _options.basedir / "branch";

    if (fs::exists(branch_path)) {
        fs::path path = _options.branchdir / _user_id.to_string();
        _branch.reset(new Branch(Branch::load(path, _user_id, _objects, _options)));
    } else {
        _branch.reset(new Branch(Branch::create(branch_path, _user_id, _objects, _options)));
    }

    std::cout << "User ID: " << _user_id << "\n";
}

net::awaitable<Repository::Attrib> Repository::get_attr(PathRange path)
{
#if DEBUG_PRINT_CALL
    std::cerr << "Enter: " << __PRETTY_FUNCTION__ << "\n";
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

    co_return _branch->branch_view().get_attr(path);
}

net::awaitable<vector<string>> Repository::readdir(PathRange path)
{
#if DEBUG_PRINT_CALL
    std::cerr << "Enter: " << __PRETTY_FUNCTION__ << "\n";
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

    std::vector<std::string> nodes;

    auto dir = _branch->branch_view().readdir(path);

    for (auto& name : dir) {
        nodes.push_back(name);
    }

    co_return nodes;
}

net::awaitable<size_t> Repository::read(PathRange path, char* buf, size_t size, off_t offset)
{
#if DEBUG_PRINT_CALL
    std::cerr << "Enter: " << __PRETTY_FUNCTION__ << "\n";
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

    if (path.empty()) {
        throw_error(sys::errc::is_a_directory);
    }

    co_return _branch->branch_view().read(path, buf, size, offset);
}

net::awaitable<size_t> Repository::write(PathRange path, const char* buf, size_t size, off_t offset)
{
#if DEBUG_PRINT_CALL
    std::cerr << "Enter: " << __PRETTY_FUNCTION__ << "\n";
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

    if (path.empty()) {
        throw_error(sys::errc::is_a_directory);
    }

    auto retval = _branch->write(path, buf, size, offset);

    _on_change.notify();

    co_return retval;
}

net::awaitable<void> Repository::mknod(PathRange path, mode_t mode, dev_t dev)
{
#if DEBUG_PRINT_CALL
    std::cerr << "Enter: " << __PRETTY_FUNCTION__ << "\n";
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

    if (S_ISFIFO(mode)) throw_error(sys::errc::invalid_argument); // TODO?

    if (path.empty()) {
        throw_error(sys::errc::is_a_directory);
    }

    _branch->store(path, FileBlob{});

    _on_change.notify();

    co_return;
}

net::awaitable<void> Repository::mkdir(PathRange path, mode_t mode)
{
#if DEBUG_PRINT_CALL
    std::cerr << "Enter: " << __PRETTY_FUNCTION__ << "\n";
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

    _branch->mkdir(path);

    _on_change.notify();
    co_return;
}

net::awaitable<void> Repository::remove_file(PathRange path)
{
#if DEBUG_PRINT_CALL
    std::cerr << "Enter: " << __PRETTY_FUNCTION__ << "\n";
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

    if (path.empty()) {
        throw_error(sys::errc::operation_not_permitted);
    }

    _branch->remove(path);

    _on_change.notify();
    co_return;
}

net::awaitable<void> Repository::remove_directory(PathRange path)
{
#if DEBUG_PRINT_CALL
    std::cerr << "Enter: " << __PRETTY_FUNCTION__ << "\n";
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

    if (path.empty()) {
        throw_error(sys::errc::operation_not_permitted);
    }

    _branch->remove(path);

    _on_change.notify();
    co_return;
}

net::awaitable<size_t> Repository::truncate(PathRange path, size_t size)
{
#if DEBUG_PRINT_CALL
    std::cerr << "Enter: " << __PRETTY_FUNCTION__ << "\n";
#endif

#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

    if (path.empty()) {
        throw_error(sys::errc::is_a_directory);
    }

    auto retval = _branch->truncate(path, size);

    _on_change.notify();
    co_return retval;
}

void Repository::sanity_check() const
{
    _branch->sanity_check();
}
