#include <algorithm>
#include <iterator>
#include <sstream>
#define BOOST_TEST_MODULE network_sockets
#include <boost/test/included/unit_test.hpp>

#include <boost/asio.hpp>
#include <boost/asio/spawn.hpp>
#include <ouisync.hpp>
#include <ouisync/service.hpp>

#include "tests/test_utils.hpp"

namespace asio = boost::asio;

template<typename T>
std::string to_string(const T& value) {
    std::ostringstream stream;
    stream << value;
    return stream.str();
}

std::vector<uint8_t> to_bytes(const std::string& str) {
    std::vector<uint8_t> out(str.size());

    std::transform(str.begin(), str.end(), out.begin(), [](char c) {
        return (uint8_t) c;
    });

    return out;
}

std::string from_bytes(const std::vector<uint8_t>& bytes) {
    std::string out;
    out.reserve(bytes.size());

    std::transform(bytes.begin(), bytes.end(), std::back_inserter(out), [](uint8_t b) {
        return (char) b;
    });

    return out;
}

BOOST_AUTO_TEST_CASE(test_ping) {
    asio::io_context ctx;

    auto tempdir = TempDir();

    asio::spawn(ctx, [&] (asio::yield_context yield) {
        ouisync::init_log();
        ouisync::Service service(yield.get_executor());
        service.start(tempdir.path(), nullptr, yield);

        auto alice_session = ouisync::Session::connect(tempdir.path(), yield);
        alice_session.bind_network({"quic/127.0.0.1:0"}, yield);

        auto bob_socket = asio::ip::udp::socket(
            yield.get_executor(),
            asio::ip::udp::endpoint(asio::ip::make_address_v4("127.0.0.1"), 0)
        );
        auto bob_endpoint = bob_socket.local_endpoint();

        auto alice_socket = alice_session.open_network_socket_v4(yield);

        // Send PING
        {
            auto sent_len = alice_socket->send_to(to_bytes("ping"), to_string(bob_endpoint), yield);
            BOOST_REQUIRE_EQUAL(sent_len, 4);
        }

        // Receive PING, send PONG
        {
            asio::ip::udp::endpoint endpoint;
            std::string data("****");
            auto len = bob_socket.async_receive_from(asio::buffer(data), endpoint, yield);
            BOOST_REQUIRE_EQUAL(len, 4);
            BOOST_REQUIRE_EQUAL(data, "ping");

            data = std::string("pong");
            len = bob_socket.async_send_to(asio::buffer(data), endpoint, yield);
            BOOST_REQUIRE_EQUAL(len, 4);
        }

        // Receive PONG
        {
            auto datagram = alice_socket->recv_from(4, yield);
            BOOST_REQUIRE_EQUAL(from_bytes(datagram.data), "pong");
            BOOST_REQUIRE_EQUAL(datagram.addr, to_string(bob_endpoint));
        }

        service.stop(yield);

    }, check_exception);

    ctx.run();
}
