#define BOOST_TEST_MODULE file
#include <boost/test/included/unit_test.hpp>

#include <boost/asio.hpp>
#include <ouisync.hpp>
#include <ouisync/service.hpp>
#include <ouisync/file_stream.hpp>

#include "tests/test_utils.hpp"

namespace asio = boost::asio;

std::tuple<TempDir, ouisync::Service, ouisync::Session, ouisync::Repository>
setup(asio::yield_context yield) {
    auto tempdir = TempDir();
    auto config_dir = mkdir(tempdir.path() / "config");
    auto store_dir = mkdir(tempdir.path() / "store");

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

    return std::make_tuple(
        std::move(tempdir),
        std::move(service),
        std::move(session),
        std::move(repo)
    );
}

BOOST_AUTO_TEST_CASE(test_file) {
    asio::io_context ctx;

    auto tempdir = TempDir();
    auto config_dir = mkdir(tempdir.path() / "config");
    auto store_dir = mkdir(tempdir.path() / "store");

    asio::spawn(ctx, [&] (asio::yield_context yield) {
        auto [tempdir, service, session, repo] = setup(yield);

        auto file_w = repo.create_file("test.txt", yield);
        file_w.write(0, to_bytes("hello world"), yield);
        file_w.close(yield);

        auto file_r = repo.open_file("test.txt", yield);
        auto content = from_bytes(file_r.read(0, 11, yield));
        file_r.close(yield);

        BOOST_REQUIRE_EQUAL(content, "hello world");

        service.stop(yield);
    }, check_exception);

    ctx.run();
}

BOOST_AUTO_TEST_CASE(test_file_stream) {
    asio::io_context ctx;

    asio::spawn(ctx, [&] (asio::yield_context yield) {
        auto [tempdir, service, session, repo] = setup(yield);

        auto file_w = repo.create_file("test.txt", yield);
        auto stream_w = ouisync::FileStream::init(std::move(file_w), yield);
        auto n_w = asio::async_write(stream_w, asio::buffer(std::string("hello world")), yield);
        BOOST_REQUIRE_EQUAL(n_w, 11);

        stream_w.close(yield);

        auto file_r = repo.open_file("test.txt", yield);
        auto stream_r = ouisync::FileStream::init(std::move(file_r), yield);

        std::vector<uint8_t> buffer(11);
        auto n_r = asio::async_read(stream_r, asio::buffer(buffer), yield);
        BOOST_REQUIRE_EQUAL(n_r, 11);
        BOOST_REQUIRE_EQUAL(from_bytes(buffer), "hello world");

        stream_r.close(yield);

        service.stop(yield);
    }, check_exception);

    ctx.run();

}
