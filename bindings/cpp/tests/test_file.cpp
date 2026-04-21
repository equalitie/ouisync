#define BOOST_TEST_MODULE file
#include <boost/test/included/unit_test.hpp>

#include <ouisync.hpp>
#include <ouisync/service.hpp>

#include "tests/test_utils.hpp"

namespace asio = boost::asio;

BOOST_AUTO_TEST_CASE(test_basic_operations) {
    asio::io_context ctx;

    auto tempdir = TempDir();
    auto config_dir = mkdir(tempdir.path() / "config");
    auto store_dir = mkdir(tempdir.path() / "store");

    asio::spawn(ctx, [&] (asio::yield_context yield) {
        ouisync::init_log();
        ouisync::Service service(yield.get_executor());
        service.start(config_dir, nullptr, yield);

        auto session = ouisync::Session::connect(config_dir, yield);
        session.set_store_dirs({ store_dir.string() }, yield);

        auto repo = session.create_repository(
            "my-repo",
            std::nullopt,
            std::nullopt,
            std::nullopt,
            false,
            false,
            false,
            yield
        );

        auto file_w = repo.create_file("test.txt", yield);
        file_w.write(0, to_bytes("hello world"), yield);
        file_w.close(yield);

        auto file_r = repo.open_file("test.txt", yield);
        auto content = from_bytes(file_r.read(0, 11, yield));
        file_r.close(yield);

        BOOST_REQUIRE_EQUAL(content, "hello world");

        service.stop(yield);
        // ...
    }, check_exception);

    ctx.run();
}
