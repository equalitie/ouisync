#define BOOST_TEST_MODULE dht
#include <boost/test/included/unit_test.hpp>

#include <boost/asio.hpp>
#include <boost/asio/spawn.hpp>
#include <ouisync.hpp>
#include <ouisync/service.hpp>

#include "tests/test_utils.hpp"

namespace asio = boost::asio;
BOOST_AUTO_TEST_CASE(test_cancellation) {
    asio::io_context ctx;

    auto tempdir = TempDir();

    asio::spawn(ctx, [&] (asio::yield_context yield) {
        ouisync::init_log();

        auto config_dir = mkdir(tempdir.path() / "config");

        ouisync::Service service(yield.get_executor());
        service.start(config_dir, nullptr, yield);

        auto session = ouisync::Session::connect(config_dir, yield);
        session.set_store_dirs({ mkdir(tempdir.path() / "store").string() }, yield);

        // invoke the operation without waiting for its result immediatelly. Use this channel to
        // retrieve the result later.
        asio::experimental::channel<void(boost::system::error_code, ouisync::Repository)> channel(ctx, 0);

        asio::cancellation_signal cancellation_signal;
        auto cancellation_slot = cancellation_signal.slot();

        session.create_repository(
            "my-repo",
            std::nullopt,
            std::nullopt,
            std::nullopt,
            false,
            false,
            false,
            asio::bind_cancellation_slot(
                cancellation_slot,
                [&](boost::system::error_code ec, ouisync::Repository repo) {
                    channel.async_send(ec, std::move(repo), asio::detached);
                }
            )
        );

        cancellation_signal.emit(asio::cancellation_type::total);

        boost::system::error_code ec;
        channel.async_receive(yield[ec]);

        BOOST_REQUIRE_EQUAL(ec, asio::error::operation_aborted);

    }, check_exception);

    ctx.run();
}
