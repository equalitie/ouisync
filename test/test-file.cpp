#define BOOST_TEST_MODULE file
#include <boost/test/included/unit_test.hpp>

#include "../src/shortcuts.h"
#include "../src/file.h"
#include "../src/file_locker.h"

#include <iostream>
#include <boost/asio/co_spawn.hpp>
#include <boost/asio/detached.hpp>
#include <boost/filesystem/path.hpp>
#include <boost/filesystem/operations.hpp>
#include <boost/filesystem/fstream.hpp>

using namespace std;
using namespace ouisync;

fs::path choose_test_dir(string subdir) {
    static auto basedir = fs::unique_path("/tmp/ouisync/test-objects-%%%%-%%%%-%%%%-%%%%");
    auto dir = basedir / subdir;
    fs::create_directories(dir);
    return dir;
}

template<class AsyncFunc>
auto spawn(net::io_context& ioc, AsyncFunc&& async_func)
{
    co_spawn(ioc, [a = std::move(async_func)] () -> net::awaitable<void> {
            try {
                co_await a();
            }
            catch(const std::exception& e) {
                std::cerr << "Exception: " << e.what() << "\n";
                BOOST_REQUIRE(false && "Exception thrown");
            }
        }, net::detached);
}

BOOST_AUTO_TEST_CASE(file) {
    auto testdir = choose_test_dir("file");

    net::io_context ioc;

    spawn(ioc, [&] () -> net::awaitable<void> {
        auto testfile = testdir/"test.txt";

        const string src_str = "test";
        string dst_str(src_str.size(), '\0');

        {
            auto file = File::truncate_or_create(ioc, testfile);
            co_await file.write(net::buffer(src_str), {});
        }

        {
            auto file = File::open_readonly(ioc, testfile);
            co_await file.read(net::buffer(dst_str), {});
        }

        BOOST_REQUIRE_EQUAL(src_str, dst_str);
    });

    ioc.run();
}

BOOST_AUTO_TEST_CASE(file_locker) {
    auto testdir = choose_test_dir("file_locker");

    net::io_context ioc;

    spawn(ioc, [&] () -> net::awaitable<void> {
        FileLocker locker(ioc);

        auto path = testdir/"f";
        const string src_str = "test";

        {
            auto f = co_await locker.truncate_or_create(path, {});
            co_await f.write(net::buffer(src_str), {});
        }

        {
            string dst_str(src_str.size(), '\0');
            auto f = co_await locker.open_readonly(path, {});
            co_await f.read(net::buffer(dst_str), {});

            BOOST_REQUIRE_EQUAL(src_str, dst_str);
        }
    });

    ioc.run();
}

static void create_file(fs::path path, const char* content) {
    // Just touch the file so we can open it later.
    fs::ofstream f(path, f.out | f.binary | f.trunc);
    f << content;
}

// Test that opening a second file for reading doesn't block
BOOST_AUTO_TEST_CASE(file_locker_read_nowait) {
    auto testdir = choose_test_dir("file_locker_read_nowait");

    net::io_context ioc;

    FileLocker locker(ioc);

    auto path = testdir/"f";
    create_file(path, "test");

    Barrier barrier(ioc);
    Opt<Barrier::Lock> lock = barrier.lock();

    unsigned step = 0;

    spawn(ioc, [&] () -> net::awaitable<void> {
        BOOST_REQUIRE_EQUAL(step++, 0);
        auto f = co_await locker.open_readonly(path, {});
        BOOST_REQUIRE_EQUAL(step++, 1);
        co_await barrier.wait({});
        BOOST_REQUIRE_EQUAL(step++, 4);
    });

    spawn(ioc, [&] () -> net::awaitable<void> {
        BOOST_REQUIRE_EQUAL(step++, 2);
        lock = boost::none;
        auto f = co_await locker.open_readonly(path, {});
        BOOST_REQUIRE_EQUAL(step++, 3);
    });

    ioc.run();
}

// Test that opening a second file for writing does block
BOOST_AUTO_TEST_CASE(file_locker_write_wait) {
    auto testdir = choose_test_dir("file_locker_write_wait");

    net::io_context ioc;

    FileLocker locker(ioc);

    auto path = testdir/"f";

    Barrier barrier(ioc);

    Opt<Barrier::Lock> lock = barrier.lock();

    unsigned step = 0;

    spawn(ioc, [&] () -> net::awaitable<void> {
        BOOST_REQUIRE_EQUAL(step++, 0);
        auto f = co_await locker.truncate_or_create(path, {});
        BOOST_REQUIRE_EQUAL(step++, 1);
        co_await barrier.wait({});
        BOOST_REQUIRE_EQUAL(step++, 3);
    });

    spawn(ioc, [&] () -> net::awaitable<void> {
        BOOST_REQUIRE_EQUAL(step++, 2);
        lock = boost::none;
        // The follwing line must wait for the file from the above coroutine to
        // go out of scope.
        auto f = co_await locker.truncate_or_create(path, {});
        BOOST_REQUIRE_EQUAL(step++, 4);
    });

    ioc.run();
}

// Test that opening a second file for writing does block
BOOST_AUTO_TEST_CASE(file_locker_read_write_wait) {
    auto testdir = choose_test_dir("file_locker_read_write_wait");

    net::io_context ioc;

    FileLocker locker(ioc);

    auto path = testdir/"f";
    create_file(path, "test");

    Barrier barrier(ioc);

    Opt<Barrier::Lock> lock = barrier.lock();

    unsigned step = 0;

    spawn(ioc, [&] () -> net::awaitable<void> {
        BOOST_REQUIRE_EQUAL(step++, 0);
        auto f = co_await locker.open_readonly(path, {});
        BOOST_REQUIRE_EQUAL(step++, 1);
        co_await barrier.wait({});
        BOOST_REQUIRE_EQUAL(step++, 3);
    });

    spawn(ioc, [&] () -> net::awaitable<void> {
        BOOST_REQUIRE_EQUAL(step++, 2);
        lock = boost::none;
        // The follwing line must wait for the file from the above coroutine to
        // go out of scope.
        auto f = co_await locker.truncate_or_create(path, {});
        BOOST_REQUIRE_EQUAL(step++, 4);
    });

    ioc.run();
}
