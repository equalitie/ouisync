#define BOOST_TEST_MODULE utils
#include <boost/test/included/unit_test.hpp>

#include "../src/namespaces.h"
#include "../src/block.h"

#include <boost/asio.hpp>
//#include <iostream>

using namespace std;
using namespace ouisync;

template<class Awaitable>
void spawn(net::io_context& ioc, Awaitable&& awaitable) {
    co_spawn(ioc, std::move(awaitable), net::detached);
}

BOOST_AUTO_TEST_CASE(block) {
    net::io_context ioc;

    spawn(ioc, [&] () mutable -> net::awaitable<void> {
        Block block(ioc);

        bool spawn_is_running = true;

        spawn(ioc, [&, lock = block.lock()] () mutable -> net::awaitable<void> {
            co_await net::post(ioc, net::use_awaitable);
            spawn_is_running = false;
        });

        BOOST_REQUIRE(spawn_is_running);

        co_await block.wait();

        BOOST_REQUIRE(!spawn_is_running);
    });

    ioc.run();
}
