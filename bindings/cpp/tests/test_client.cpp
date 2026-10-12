#define BOOST_TEST_MODULE client
#include <boost/test/included/unit_test.hpp>

#include <boost/asio.hpp>
#include <boost/asio/spawn.hpp>
#include <boost/endian/conversion.hpp>
#include <boost/filesystem/fstream.hpp>
#include <ouisync.hpp>
#include <ouisync/service.hpp>
#include <random>

#include "tests/test_utils.hpp"

namespace asio = boost::asio;
namespace fs = boost::filesystem;

static void sanity_check(const fs::path& config_dir) {
    asio::io_context ctx;

    asio::spawn(ctx, [&] (asio::yield_context yield) {
        ouisync::Service service(yield.get_executor());
        service.start(config_dir, nullptr, yield);

        auto session = ouisync::Session::connect(config_dir, yield);
        auto store_dirs = session.get_store_dirs(yield);
        BOOST_REQUIRE(store_dirs.empty());

        service.stop(yield);
    }, check_exception);

    ctx.run();
}

static std::string random_hex(size_t size) {
    static const char alphabet[] = "0123456789abcdef";

    std::random_device dev;
    std::mt19937 rng(dev());
    std::uniform_int_distribution<size_t> dist(0, 15);

    std::string out(size, '0');
    std::generate(out.begin(), out.end(), [&] { return alphabet[dist(rng)]; });
    return out;
}

// Run sanity check with the default API protocol transport (unix domain socket on platforms that
// support it, TCP on loopback otherwise)
BOOST_AUTO_TEST_CASE(sanity_check_default) {
    ouisync::init_log();

    auto tempdir = TempDir();
    sanity_check(tempdir.path() / "config");
}

// Run sanity check with TCP on loopback as the API protocol transport.
BOOST_AUTO_TEST_CASE(sanity_check_tcp) {
    auto tempdir = TempDir();
    auto config_dir = mkdir(tempdir.path() / "config");

    fs::ofstream(config_dir / "local_endpoint.conf")
        << "\"tcp://127.0.0.1:0?auth_key=" << random_hex(64) << "\"";

    sanity_check(config_dir);
}

#if defined(BOOST_ASIO_HAS_LOCAL_SOCKETS)
// Run sanity check with unix domain socket at custom location as the API protocol transport.
BOOST_AUTO_TEST_CASE(sanity_check_unix_custom_path) {
    auto tempdir = TempDir();
    auto config_dir = mkdir(tempdir.path() / "config");
    auto socket_dir = mkdir(tempdir.path() / "sockets");

    fs::ofstream(config_dir / "local_endpoint.conf")
        << "\"unix://" << (socket_dir / "ouisync.sock").string() << "\"";

    sanity_check(config_dir);

    BOOST_REQUIRE(!fs::exists(config_dir / "local_endpoint.sock"));
}
#endif

// Connecting to TCP service address with missing or zero port must fail.
BOOST_AUTO_TEST_CASE(connect_tcp_invalid_port) {
    auto tempdir = TempDir();
    auto config_dir = mkdir(tempdir.path() / "config");

    for (std::string endpoint : { "127.0.0.1", "127.0.0.1:0", "[::1]", "::1", "[::1]:0" }) {
        BOOST_TEST_CONTEXT("endpoint: " << endpoint) {
            fs::ofstream(config_dir / "local_endpoint.conf")
                << "\"tcp://" << endpoint << "?auth_key=" << random_hex(64) << "\"";

            asio::io_context ctx;

            asio::spawn(ctx, [&] (asio::yield_context yield) {
                boost::system::error_code ec;
                ouisync::Session::connect(config_dir, yield[ec]);
                BOOST_CHECK_EQUAL(ec, ouisync::error::service_endpoint_invalid);
            }, check_exception);

            ctx.run();
        }
    }
}

// Connecting when the service config doesn't exist must fail with an error that can be caught
// inside the coroutine.
BOOST_AUTO_TEST_CASE(connect_missing_config) {
    auto tempdir = TempDir();
    auto config_dir = mkdir(tempdir.path() / "config");

    asio::io_context ctx;

    asio::spawn(ctx, [&] (asio::yield_context yield) {
        BOOST_CHECK_EXCEPTION(
            ouisync::Session::connect(config_dir, yield),
            boost::system::system_error,
            [](const auto& e) { return e.code() == ouisync::error::service_endpoint_not_found; }
        );
    }, check_exception);

    ctx.run();
}

// Connecting to a service that uses a different API protocol version must fail.
BOOST_AUTO_TEST_CASE(connect_unsupported_protocol_version) {
    namespace endian = boost::endian;
    using tcp = asio::ip::tcp;

    auto tempdir = TempDir();
    auto config_dir = mkdir(tempdir.path() / "config");

    asio::io_context ctx;

    // Fake service which responds with an unsupported protocol version.
    tcp::acceptor acceptor(ctx, tcp::endpoint(asio::ip::address_v4::loopback(), 0));

    fs::ofstream(config_dir / "local_endpoint.conf")
        << "\"tcp://127.0.0.1:" << acceptor.local_endpoint().port()
        << "?auth_key=" << random_hex(64) << "\"";

    asio::spawn(ctx, [&] (asio::yield_context yield) {
        auto socket = acceptor.async_accept(yield);

        uint64_t client_version_be = 0;
        asio::async_read(socket, asio::buffer(&client_version_be, sizeof(client_version_be)), yield);
        BOOST_CHECK_EQUAL(
            endian::big_to_native(client_version_be),
            static_cast<uint64_t>(ouisync::API_PROTOCOL_VERSION)
        );

        uint64_t server_version_be = endian::native_to_big(
            static_cast<uint64_t>(ouisync::API_PROTOCOL_VERSION + 1)
        );
        asio::async_write(socket, asio::buffer(&server_version_be, sizeof(server_version_be)), yield);
    }, check_exception);

    asio::spawn(ctx, [&] (asio::yield_context yield) {
        boost::system::error_code ec;
        ouisync::Session::connect(config_dir, yield[ec]);
        BOOST_CHECK_EQUAL(ec, ouisync::error::unsupported_protocol_version);
    }, check_exception);

    ctx.run();
}
