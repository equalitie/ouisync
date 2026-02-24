#define BOOST_TEST_MODULE utility
#include <boost/test/included/unit_test.hpp>

#include <boost/asio.hpp>
#include <boost/asio/spawn.hpp>
#include <boost/filesystem.hpp>
#include <iostream>
#include <ouisync.hpp>
#include <ouisync/service.hpp>

using namespace std;
using namespace std::chrono_literals;
using namespace boost::asio::ip;
namespace asio = boost::asio;
namespace fs = boost::filesystem;
namespace sys = boost::system;

void check_exception(std::exception_ptr e) {
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

static auto const& current_test_case() {
    return boost::unit_test::framework::current_test_case();
}

static std::string test_name() {
    return current_test_case().p_name;
}

static fs::path mkdir(fs::path path) {
    fs::create_directories(path);
    return path;
}

// We also have similar test in Rust, but this one was crashing with
// stack-verflow because the stack allocated by `asio::spawn` is too small.
BOOST_AUTO_TEST_CASE(copy_dirs) {
    asio::io_context ctx;

    fs::path tempdir = mkdir(fs::temp_directory_path() / "ouisync-cpp-tests" / test_name() / fs::unique_path());
    fs::path service_dir = mkdir(tempdir / "ouisync");
    fs::path first_dir = mkdir(tempdir / "dir1");
    fs::path dirs = mkdir(first_dir / "dir2" / "dir3");

    auto repo_name = "my_repo";

    asio::spawn(ctx, [&] (asio::yield_context yield) {
        ouisync::Service service(yield.get_executor());
        service.start(service_dir.string().c_str(), "ouisync-service", yield);

        auto session = ouisync::Session::connect(service_dir, yield);

        session.set_store_dirs({mkdir(service_dir / "store").string()}, yield);

        // Create a repo and copy the fetched content into it
        auto page_repo = session.create_repository(
                repo_name,
                {},    // read secret
                {},    // write secret
                {},    // token
                false, // sync enabled
                false, // dht enabled
                false, // pex_enabled
                yield);

        session.copy(
            {},                 // `src_repo`
            first_dir.string(), // `src_path`
            repo_name,          // `dst_repo`
            "/",                // `dst_path`
            yield);
    },
    check_exception);

    ctx.run();
}

