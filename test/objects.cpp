#define BOOST_TEST_MODULE objects
#include <boost/test/included/unit_test.hpp>

#include "object/block.h"
#include "object/load.h"
#include "namespaces.h"
#include "hex.h"
#include "array_io.h"

#include <iostream>
#include <random>
#include <boost/filesystem.hpp>
#include <boost/variant.hpp>
#include <boost/serialization/string.hpp>

using namespace std;
using namespace ouisync;

using Data = cpputils::Data;

struct Random {
    Random() : gen(std::random_device()()) {}

    Data data(size_t size) {
        Data d(size);
        auto ptr = static_cast<char*>(d.data());
        std::uniform_int_distribution<> distrib(0, 255);
        for (size_t i = 0; i < d.size(); ++i) {
            ptr[i] = distrib(gen);
        }
        return d;
    }

    std::mt19937 gen;
};


BOOST_AUTO_TEST_CASE(block_is_same) {
    fs::path testdir = fs::unique_path("/tmp/ouisync/test-objects-%%%%-%%%%-%%%%-%%%%");

    Random random;

    Data data(random.data(1000));

    object::Block b1(data);
    b1.store(testdir);

    auto b2 = object::load<object::Block>(testdir, b1.calculate_id());

    BOOST_REQUIRE_EQUAL(to_hex<char>(b1.calculate_id()), to_hex<char>(b2.calculate_id()));
}
