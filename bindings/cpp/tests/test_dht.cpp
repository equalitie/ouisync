#define BOOST_TEST_MODULE dht
#include <boost/test/included/unit_test.hpp>

#include <boost/asio.hpp>
#include <boost/asio/spawn.hpp>
#include <ouisync.hpp>
#include <ouisync/service.hpp>

#include "tests/test_utils.hpp"

namespace asio = boost::asio;

BOOST_AUTO_TEST_CASE(test_dht) {
    asio::io_context ctx;

    auto tempdir = TempDir();

    asio::spawn(ctx, [&] (asio::yield_context yield) {
        ouisync::init_log();

        ouisync::Service bootstrap_service(yield.get_executor());
        bootstrap_service.start(mkdir(tempdir.path() / "bootstrap"), nullptr, yield);

        // TODO: test
    }, check_exception);

    ctx.run();
}
