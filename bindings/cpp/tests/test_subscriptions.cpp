#define BOOST_TEST_MODULE Subscriptions
#include <boost/test/included/unit_test.hpp>

#include <boost/asio.hpp>
#include <boost/asio/spawn.hpp>
#include <ouisync.hpp>
#include <ouisync/service.hpp>
#include "tests/test_utils.hpp"

namespace asio = boost::asio;

BOOST_AUTO_TEST_CASE(network_events) {
    asio::io_context ctx;
    TempDir tempdir;

    auto a_dir = mkdir(tempdir.path() / "a");
    auto b_dir = mkdir(tempdir.path() / "b");

    asio::spawn(ctx, [&] (asio::yield_context yield) {
        ouisync::init_log();

        ouisync::Service a_service(yield.get_executor());
        a_service.start(a_dir, "a", yield);
        auto a_session = ouisync::Session::connect(a_dir, yield);
        a_session.bind_network({"quic/127.0.0.1:0"}, yield);

        ouisync::Service b_service(yield.get_executor());
        b_service.start(b_dir, "b", yield);
        auto b_session = ouisync::Session::connect(b_dir, yield);
        b_session.bind_network({"quic/127.0.0.1:0"}, yield);

        auto b_addr = b_session.get_local_listener_addrs(yield)[0];

        auto subscription = a_session.subscribe_to_network();
        a_session.add_user_provided_peers({ b_addr }, yield);

        // Known
        BOOST_REQUIRE_EQUAL(subscription.async_receive(yield), ouisync::NetworkEvent::peer_set_change);
        auto p0 = a_session.get_peers(yield)[0];
        BOOST_REQUIRE_EQUAL(p0.addr, b_addr);
        BOOST_REQUIRE(
            p0.state.get_if<ouisync::PeerState::Known>() != nullptr ||
            p0.state.get_if<ouisync::PeerState::Connecting>() != nullptr ||
            p0.state.get_if<ouisync::PeerState::Handshaking>() != nullptr ||
            p0.state.get_if<ouisync::PeerState::Active>() != nullptr
        );

        // Connecting
        BOOST_REQUIRE_EQUAL(subscription.async_receive(yield), ouisync::NetworkEvent::peer_set_change);
        auto p1 = a_session.get_peers(yield)[0];
        BOOST_REQUIRE_EQUAL(p1.addr, b_addr);
        BOOST_REQUIRE(
            p1.state.get_if<ouisync::PeerState::Connecting>() != nullptr ||
            p1.state.get_if<ouisync::PeerState::Handshaking>() != nullptr ||
            p1.state.get_if<ouisync::PeerState::Active>() != nullptr
        );

        // Handshaking
        BOOST_REQUIRE_EQUAL(subscription.async_receive(yield), ouisync::NetworkEvent::peer_set_change);
        auto p2 = a_session.get_peers(yield)[0];
        BOOST_REQUIRE_EQUAL(p2.addr, b_addr);
        BOOST_REQUIRE(
            p1.state.get_if<ouisync::PeerState::Handshaking>() != nullptr ||
            p1.state.get_if<ouisync::PeerState::Active>() != nullptr
        );

        // Active
        BOOST_REQUIRE_EQUAL(subscription.async_receive(yield), ouisync::NetworkEvent::peer_set_change);

        auto p3 = a_session.get_peers(yield)[0];
        BOOST_REQUIRE_EQUAL(p2.addr, b_addr);
        BOOST_REQUIRE(
            p1.state.get_if<ouisync::PeerState::Active>() != nullptr
        );

        a_session.remove_user_provided_peers({ b_addr }, yield);
        b_service.stop(yield);

        // Peer disconnected
        BOOST_REQUIRE_EQUAL(subscription.async_receive(yield), ouisync::NetworkEvent::peer_set_change);
        auto ps = a_session.get_peers(yield);
        BOOST_REQUIRE(ps.empty());

        a_service.stop(yield);
    }, check_exception);

    ctx.run();
}
