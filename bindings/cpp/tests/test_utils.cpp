#include <exception>

#include "test_utils.hpp"

namespace fs = boost::filesystem;

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

fs::path mkdir(const fs::path& path) {
    fs::create_directories(path);
    return path;
}
