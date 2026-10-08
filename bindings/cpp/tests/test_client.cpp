#define BOOST_TEST_MODULE client
#include <boost/test/included/unit_test.hpp>

#include <boost/asio.hpp>
#include <boost/asio/spawn.hpp>
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
                ouisync::Session::connect(config_dir, yield);
            }, check_exception);

            // NOTE: The address is parsed in the async initiation function which, with
            // `yield_context`, runs outside of the coroutine. So the error propagates out of
            // `ctx.run()` instead of from `Session::connect`.
            BOOST_CHECK_THROW(ctx.run(), boost::system::system_error);
        }
    }
}
