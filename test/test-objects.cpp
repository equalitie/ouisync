#define BOOST_TEST_MODULE objects
#include <boost/test/included/unit_test.hpp>

#include "object/blob.h"
#include "object/tree.h"
#include "object/io.h"
#include "shortcuts.h"
#include "hex.h"
#include "array_io.h"
#include "branch.h"
#include "path_range.h"

#include <iostream>
#include <random>
#include <boost/filesystem.hpp>
#include <boost/variant.hpp>
#include <boost/range/distance.hpp>
#include <boost/serialization/string.hpp>
#include <boost/serialization/vector.hpp>
#include <boost/range/adaptor/filtered.hpp>

using namespace std;
using namespace ouisync;

using object::Tree;
using object::Blob;
using object::Id;
using boost::variant;

struct Random {
    Random() : gen(std::random_device()()) {}

    std::vector<uint8_t> vector(size_t size) {
        std::vector<uint8_t> v(size);
        auto ptr = static_cast<uint8_t*>(v.data());
        fill(ptr, size);
        return v;
    }

    std::string string(size_t size) {
        std::string result(size, '\0');
        fill(&result[0], size);
        return result;
    }

    Id object_id() {
        Id id;
        fill(reinterpret_cast<char*>(id.data()), id.size());
        return id;
    }

    void fill(void* ptr, size_t size) {
        std::uniform_int_distribution<> distrib(0, 255);
        for (size_t i = 0; i < size; ++i) ((char*)ptr)[i] = distrib(gen);
    }

    std::mt19937 gen;
};

#define REQUIRE_HEX_EQUAL(a, b) \
    BOOST_REQUIRE_EQUAL(to_hex<char>(a), to_hex<char>(b));

bool is_refcount(const fs::path& path) {
    return path.extension() == ".rc";
}

auto files_in(const fs::path& path) {
    return fs::recursive_directory_iterator(path) |
        boost::adaptors::filtered([](auto p) {
                return fs::is_regular_file(p); });
}

auto objects_in(const fs::path& path) {
    return files_in(path) |
        boost::adaptors::filtered([](auto p) { return !is_refcount(p.path()); });
}

size_t count_files(const fs::path& path) {
    return boost::distance(files_in(path));
}

size_t count_objects(const fs::path& path) {
    return boost::distance(objects_in(path));
}

fs::path choose_test_dir() {
    return fs::unique_path("/tmp/ouisync/test-objects-%%%%-%%%%-%%%%-%%%%");
}

BOOST_AUTO_TEST_CASE(blob_is_same) {
    fs::path testdir = choose_test_dir();

    Random random;
    object::Blob b1(random.vector(1000));
    //b1.store(testdir);
    object::io::store(testdir, b1);
    auto b2 = object::io::load<object::Blob>(testdir, object::calculate_id(b1));
    REQUIRE_HEX_EQUAL(object::calculate_id(b1), object::calculate_id(b2));
}

BOOST_AUTO_TEST_CASE(tree_is_same) {
    fs::path testdir = choose_test_dir();

    Random random;
    object::Tree t1;
    t1[random.string(2)]  = random.object_id();
    t1[random.string(10)] = random.object_id();
    object::io::store(testdir, t1);
    auto t2 = object::io::load<object::Tree>(testdir, object::calculate_id(t1));
    REQUIRE_HEX_EQUAL(object::calculate_id(t1), object::calculate_id(t2));
}

BOOST_AUTO_TEST_CASE(read_tag) {
    fs::path testdir = choose_test_dir();

    Random random;
    Blob blob = random.vector(256);
    auto id = object::io::store(testdir, blob);
    try {
        object::io::load<object::JustTag<Blob>>(testdir, id);
    } catch (...) {
        BOOST_REQUIRE(false);
    }
}

Branch create_branch(const fs::path testdir, const char* user_id_file_name) {
    fs::path objdir = testdir/"objects";
    fs::path branchdir = testdir/"branches";

    fs::create_directories(objdir);
    fs::create_directories(branchdir);

    UserId user_id = UserId::load_or_create(testdir/user_id_file_name);

    return Branch::load_or_create(branchdir, objdir, user_id);
}

BOOST_AUTO_TEST_CASE(tree_branch_store_and_load) {
    fs::path testdir = choose_test_dir();

    Random random;

    Blob d1(random.vector(1000));

    Branch branch = create_branch(testdir, "user_id");

    branch.store("foo/bar", d1);

    BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 3 /* root + foo + bar */);

    auto od2 = branch.maybe_load("foo/bar");

    BOOST_REQUIRE(od2);
    BOOST_REQUIRE_EQUAL(d1, *od2);
}

BOOST_AUTO_TEST_CASE(branch_directories) {
    fs::path testdir = choose_test_dir();

    {
        Branch branch = create_branch(testdir/"1", "user_id");
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 1);

        branch.mkdir(path_range("dir"));
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 2);
    }

    {
        Branch branch = create_branch(testdir/"2", "user_id");
        auto empty_root_id = branch.root_object_id();

        branch.mkdir(path_range("dir"));
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 2);

        branch.rmdir(path_range("dir"));
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 1);

        BOOST_REQUIRE_EQUAL(branch.root_object_id(), empty_root_id);
    }
}

BOOST_AUTO_TEST_CASE(tree_remove) {
    fs::path testdir = choose_test_dir();

    Random random;

    // Delete data from root
    {
        auto data = random.vector(256);

        Branch branch = create_branch(testdir/"1", "user_id");
        branch.store("data", data);

        Tree root = object::io::load<Tree>(branch.object_directory(), branch.root_object_id());

        BOOST_REQUIRE_EQUAL(root.size(), 1);
        BOOST_REQUIRE_EQUAL(root.begin()->first, "data");
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 2);

        Blob blob = object::io::load<Blob>(branch.object_directory(), root.begin()->second);
        BOOST_REQUIRE_EQUAL(data, blob);

        bool removed = branch.remove("data");
        BOOST_REQUIRE(removed);

        root = object::io::load<Tree>(branch.object_directory(), branch.root_object_id());

        BOOST_REQUIRE_EQUAL(root.size(), 0);
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 1);
    }

    // Delete data from subdir, then delete the subdir
    {
        Branch branch = create_branch(testdir/"2", "user_id");

        auto data = random.vector(256);
        branch.store("dir/data", data);

        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 3);

        bool removed = branch.remove("dir/data");
        BOOST_REQUIRE(removed);

        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 2);

        removed = branch.remove("dir");
        BOOST_REQUIRE(removed);

        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 1);
    }

    // Delete subdir, check data is deleted with it
    {
        Branch branch = create_branch(testdir/"3", "user_id");

        auto data = random.vector(256);
        branch.store("dir/data", data);

        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 3);

        bool removed = branch.remove("dir");
        BOOST_REQUIRE(removed);

        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 1);
    }

    // Delete data from one root, preserve (using refcount) in the other
    {
        Branch branch1 = create_branch(testdir/"4", "user1_id");
        Branch branch2 = create_branch(testdir/"4", "user2_id");

        BOOST_REQUIRE_EQUAL(branch1.object_directory(), branch2.object_directory());

        auto data  = random.vector(256);

        branch1.store("data", data);
        branch1.store("other_data", random.vector(256));

        branch2.store("data", data);
        branch2.store("other_data", random.vector(256));

        BOOST_REQUIRE_EQUAL(count_objects(branch1.object_directory()),
                2 /* roots */ + 3 /* data */);

        branch1.remove("data");

        // Same count as before, since "data" is still in branch2
        BOOST_REQUIRE_EQUAL(count_objects(branch1.object_directory()),
                2 /* roots */ + 3 /* data */);

        branch2.remove("data");

        // Now "data" should have been removed all together
        BOOST_REQUIRE_EQUAL(count_objects(branch1.object_directory()),
                2 /* roots */ + 2 /* data */);
    }
}
