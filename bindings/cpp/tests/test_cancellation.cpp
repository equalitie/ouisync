#define BOOST_TEST_MODULE cancellation
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
        session.bind_network({"quic/127.0.0.1:0"}, yield);
        auto socket = session.open_network_socket_v4(yield);

        // Invoke the operation without waiting for its result immediatelly but use this channel to
        // retrieve the result later.
        asio::experimental::channel<void(boost::system::error_code, ouisync::Datagram)> channel(ctx, 0);

        asio::cancellation_signal cancellation_signal;
        auto cancellation_slot = cancellation_signal.slot();

        // Try to receive data on the raw UDP socket. No one is sending anything to us at this point
        // so this should wait indefinitely.
        socket.recv_from(
            1024,
            asio::bind_cancellation_slot(
                cancellation_slot,
                [&](boost::system::error_code ec, ouisync::Datagram datagram) {
                    channel.async_send(ec, std::move(datagram), asio::detached);
                }
            )
        );

        // The operation should still be ongoing. Cancel it.
        cancellation_signal.emit(asio::cancellation_type::total);

        boost::system::error_code ec;
        channel.async_receive(yield[ec]);

        BOOST_REQUIRE_EQUAL(ec, asio::error::operation_aborted);

    }, check_exception);

    ctx.run();
}
