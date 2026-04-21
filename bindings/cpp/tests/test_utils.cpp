#include <algorithm>
#include <exception>
#include <iterator>

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
