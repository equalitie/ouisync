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

static void check_exception(std::exception_ptr e) {
    try {
        if (e) {
            std::rethrow_exception(e);
        }
    } catch (const std::exception& e) {
        BOOST_FAIL("Test failed with exception: " << e.what());
    } catch (...) {
        BOOST_FAIL("Test failed with unknown exception");
    }
}

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

        auto session = ouisync::Session::connect(tempdir.path(), yield);
        session.bind_network({"quic/127.0.0.1:0"}, yield);

        auto peer_socket = asio::ip::udp::socket(
            yield.get_executor(),
            asio::ip::udp::endpoint(asio::ip::make_address_v4("127.0.0.1"), 0)
        );
        auto peer_endpoint = peer_socket.local_endpoint();

        auto session_socket = session.open_network_socket_v4(yield);

        // Send PING
        {
            auto sent_len = session_socket->send_to(to_bytes("ping"), to_string(peer_endpoint), yield);
            BOOST_REQUIRE_EQUAL(sent_len, 4);
        }

        // Receive PING, send PONG
        {
            asio::ip::udp::endpoint sender_endpoint;
            std::string recv_data(4, ' ');
            auto recv_len = peer_socket.async_receive_from(asio::buffer(recv_data), sender_endpoint, yield);
            BOOST_REQUIRE_EQUAL(recv_len, 4);
            BOOST_REQUIRE_EQUAL(recv_data, "ping");

            auto sent_len = peer_socket.async_send_to(asio::buffer(std::string("pong")), sender_endpoint, yield);
            BOOST_REQUIRE_EQUAL(sent_len, 4);
        }

        // Receive PONG
        {
            auto datagram = session_socket->recv_from(4, yield);
            BOOST_REQUIRE_EQUAL(from_bytes(datagram.data), "pong");
        }

        service.stop(yield);

    }, check_exception);

    ctx.run();
}
