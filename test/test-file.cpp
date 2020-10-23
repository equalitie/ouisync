#define BOOST_TEST_MODULE file
#include <boost/test/included/unit_test.hpp>

#include "../src/namespaces.h"
#include "../src/file.h"

#include <iostream>
#include <boost/asio/co_spawn.hpp>
#include <boost/asio/detached.hpp>
#include <boost/filesystem/path.hpp>
#include <boost/filesystem/operations.hpp>

using namespace std;
using namespace ouisync;

fs::path choose_test_dir() {
    return fs::unique_path("/tmp/ouisync/test-objects-%%%%-%%%%-%%%%-%%%%");
}

template<class Awaitable>
auto spawn(net::io_context& ioc, Awaitable&& awaitable)
{
    co_spawn(ioc, std::move(awaitable), net::detached);
}

BOOST_AUTO_TEST_CASE(file) {
    auto testdir = choose_test_dir();
    fs::create_directories(testdir);

    net::io_context ioc;

    spawn(ioc, [&] () -> net::awaitable<void> { try {
        Cancel cancel;
        auto testfile = testdir/"test.txt";

        const string src_str = "test";
        string dst_str(src_str.size(), '\0');

        {
            auto file = File::truncate_or_create(ioc, testfile);
            co_await file.write(net::buffer(src_str), cancel);
        }

        {
            auto file = File::open_readonly(ioc, testfile);
            co_await file.read(net::buffer(dst_str), cancel);
        }

        BOOST_REQUIRE_EQUAL(src_str, dst_str);
    }
    catch(const std::exception& e) {
        std::cerr << "Exception: " << e.what() << "\n";
        BOOST_REQUIRE(false && "Exception thrown");
    }});

    try {
        ioc.run();
    } catch(const std::exception& e) {
        std::cerr << "Exception: " << e.what() << "\n";
        BOOST_REQUIRE(false && "Exception thrown");
    }
}
