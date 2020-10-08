#define BOOST_TEST_MODULE objects
#include <boost/test/included/unit_test.hpp>
#include <iostream>

#include "object/block.h"
#include "object/load.h"
#include "namespaces.h"
#include "hex.h"
#include "array_io.h"

#include <boost/filesystem.hpp>
#include <boost/variant.hpp>
#include <boost/serialization/string.hpp>

using namespace std;
using namespace ouisync;

using Data = cpputils::Data;

static Data _test_data() {
    Data d(256);
    for (size_t i = 0; i < d.size(); ++i) {
        static_cast<char*>(d.data())[i] = char(i);
    }
    return d;
}

BOOST_AUTO_TEST_CASE(block) {
    fs::path testdir = fs::unique_path("/tmp/ouisync/test-objects-%%%%-%%%%-%%%%-%%%%");

    Data data(_test_data());

    object::Block b1(data);
    b1.store(testdir);

    auto b2 = object::load<object::Block>(testdir, b1.calculate_id());

    BOOST_REQUIRE_EQUAL(to_hex<char>(b1.calculate_id()), to_hex<char>(b2.calculate_id()));
}
