#define BOOST_TEST_MODULE utils
#include <boost/test/included/unit_test.hpp>

#include "../src/shortcuts.h"
#include "../src/barrier.h"
#include "../src/bouncer.h"

#include <boost/asio.hpp>
//#include <iostream>

using namespace std;
using namespace ouisync;

template<class Awaitable>
void spawn(net::io_context& ioc, Awaitable&& awaitable) {
    co_spawn(ioc, std::move(awaitable), net::detached);
}

BOOST_AUTO_TEST_CASE(cancel) {
    {
        Cancel c;

        bool canceled = false;
        auto con = c.connect([&] { canceled = true; });

        c();

        BOOST_REQUIRE(canceled);
    }
    {
        Cancel c1;
        Cancel c2(c1);

        bool canceled = false;
        auto con = c2.connect([&] { canceled = true; });

        c1();

        BOOST_REQUIRE(canceled);
    }
}

BOOST_AUTO_TEST_CASE(barrier) {
    net::io_context ioc;

    spawn(ioc, [&] () mutable -> net::awaitable<void> {
        Barrier barrier(ioc);

        bool spawn_is_running = true;

        spawn(ioc, [&, lock = barrier.lock()] () mutable -> net::awaitable<void> {
            co_await net::post(ioc, net::use_awaitable);
            spawn_is_running = false;
        });

        BOOST_REQUIRE(spawn_is_running);

        co_await barrier.wait();

        BOOST_REQUIRE(!spawn_is_running);
    });

    ioc.run();
}

BOOST_AUTO_TEST_CASE(barrier_cancel) {
    net::io_context ioc;

    spawn(ioc, [&] () mutable -> net::awaitable<void> {
        Barrier barrier(ioc);

        Cancel cancel;
        auto done = make_shared<bool>(false);

        spawn(ioc, [&, done, lock = barrier.lock()] () mutable -> net::awaitable<void> {
            while (!*done) {
                co_await net::post(ioc, net::use_awaitable);
            }
        });

        spawn(ioc, [&] () mutable -> net::awaitable<void> {
            co_await net::post(ioc, net::use_awaitable);
            cancel();
        });

        try {
            co_await barrier.wait(cancel);
        }
        catch(const sys::system_error& e) {
            BOOST_REQUIRE_EQUAL(e.code(), net::error::operation_aborted);
            *done = true;
        }
    });

    ioc.run();
}

BOOST_AUTO_TEST_CASE(bouncer) {
    net::io_context ioc;

    const unsigned N = 10;

    spawn(ioc, [&] () mutable -> net::awaitable<void> {
        Barrier barrier(ioc);
        Bouncer bouncer(ioc);

        bool in_protected_area = false;

        spawn(ioc, [&, lock = barrier.lock()] () mutable -> net::awaitable<void> {
            co_await net::post(ioc, net::use_awaitable);
            auto bouncer_lock = co_await bouncer.wait({});

            BOOST_REQUIRE(!in_protected_area);
            in_protected_area = true;

            for (auto i = 0u; i < N; ++i) {
                co_await net::post(ioc, net::use_awaitable);
            }

            in_protected_area = false;
        });

        spawn(ioc, [&, lock = barrier.lock()] () mutable -> net::awaitable<void> {
            co_await net::post(ioc, net::use_awaitable);
            auto bouncer_lock = co_await bouncer.wait({});

            BOOST_REQUIRE(!in_protected_area);
            in_protected_area = true;

            for (auto i = 0u; i < N; ++i) {
                co_await net::post(ioc, net::use_awaitable);
            }

            in_protected_area = false;
        });

        co_await barrier.wait();
    });

    ioc.run();
}
