#define BOOST_TEST_MODULE objects
#include <boost/test/included/unit_test.hpp>

#include "object/blob.h"
#include "object/tree.h"
#include "object/io.h"
#include "refcount.h"
#include "shortcuts.h"
#include "hex.h"
#include "array_io.h"
#include "local_branch.h"
#include "path_range.h"
#include "random.h"

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
using boost::variant;

struct Random {
    using Seed = std::mt19937::result_type;

    static Seed seed() {
        return std::random_device()();
    }

    Random() : gen(seed()) {}

    std::vector<uint8_t> vector(size_t size) {
        std::vector<uint8_t> v(size);
        auto ptr = static_cast<uint8_t*>(v.data());
        fill(ptr, size);
        return v;
    }

    Blob blob(size_t size) {
        return {vector(size)};
    }

    std::string string(size_t size) {
        std::string result(size, '\0');
        fill(&result[0], size);
        return result;
    }

    ObjectId object_id() {
        ObjectId id;
        fill(reinterpret_cast<char*>(id.data()), id.size);
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

size_t refcount(LocalBranch& b, const fs::path path) {
    auto id = b.id_of(path_range(path));
    return ouisync::refcount::read_recursive(b.object_directory(), id);
};

BOOST_AUTO_TEST_CASE(blob_id_calculation) {
    Random random;
    auto data1 = random.blob(256);
    auto data2 = random.blob(256);
    BOOST_REQUIRE_NE(data1, data2);
    BOOST_REQUIRE_NE(data1.calculate_id(), data2.calculate_id());
}

BOOST_AUTO_TEST_CASE(blob_is_same) {
    fs::path testdir = choose_test_dir();

    Random random;
    object::Blob b1(random.vector(1000));
    //b1.store(testdir);
    object::io::store(testdir, b1);
    auto b2 = object::io::load<object::Blob>(testdir, b1.calculate_id());
    REQUIRE_HEX_EQUAL(b1.calculate_id(), b2.calculate_id());
}

BOOST_AUTO_TEST_CASE(tree_is_same) {
    fs::path testdir = choose_test_dir();

    Random random;
    object::Tree t1;
    t1[random.string(2)].set_id(random.object_id());
    t1[random.string(10)].set_id(random.object_id());
    object::io::store(testdir, t1);
    auto t2 = object::io::load<object::Tree>(testdir, t1.calculate_id());
    REQUIRE_HEX_EQUAL(t1.calculate_id(), t2.calculate_id());
}

BOOST_AUTO_TEST_CASE(read_tag) {
    fs::path testdir = choose_test_dir();

    Random random;
    Blob blob = random.vector(256);
    auto id = object::io::store(testdir, blob);
    try {
        object::io::load<Blob::Nothing>(testdir, id);
    } catch (...) {
        BOOST_REQUIRE(false);
    }
}

LocalBranch create_branch(const fs::path testdir, const char* user_id_file_name) {
    fs::path objdir = testdir/"objects";
    fs::path branchdir = testdir/"branches";

    fs::create_directories(objdir);
    fs::create_directories(branchdir);

    UserId user_id = UserId::load_or_create(testdir/user_id_file_name);

    Random random;
    auto name = to_hex(random.string(16));
    return LocalBranch::create(branchdir/name, objdir, user_id);
}

BOOST_AUTO_TEST_CASE(branch_directories) {
    fs::path testdir = choose_test_dir();

    {
        LocalBranch branch = create_branch(testdir/"1", "user_id");
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 1);

        branch.mkdir(Path("dir"));
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 2);
        BOOST_REQUIRE_EQUAL(::refcount(branch, "dir"), 1);
    }

    {
        LocalBranch branch = create_branch(testdir/"2", "user_id");
        auto empty_root_id = branch.root_id();

        branch.mkdir(Path("dir"));
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 2);
        BOOST_REQUIRE_EQUAL(::refcount(branch, "dir"), 1);

        branch.remove(Path("dir"));
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 1);

        BOOST_REQUIRE_EQUAL(branch.root_id(), empty_root_id);
    }
}

BOOST_AUTO_TEST_CASE(tree_branch_store_and_load) {
    fs::path testdir = choose_test_dir();

    Random random;

    Blob d1(random.vector(1000));

    LocalBranch branch = create_branch(testdir, "user_id");

    branch.store("bar", d1);

    BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 2 /* root + bar */);

    auto od2 = branch.immutable_io().maybe_load(Path("bar"));

    BOOST_REQUIRE(od2);
    BOOST_REQUIRE_EQUAL(d1, *od2);
}

BOOST_AUTO_TEST_CASE(tree_branch_store_and_load_in_subdir) {
    fs::path testdir = choose_test_dir();

    Random random;

    Blob d1(random.vector(1000));

    LocalBranch branch = create_branch(testdir, "user_id");

    branch.mkdir(Path("foo"));
    BOOST_REQUIRE_EQUAL(::refcount(branch, "foo"), 1);

    branch.store("foo/bar", d1);

    BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 3 /* root + foo + bar */);

    auto od2 = branch.immutable_io().maybe_load(Path("foo/bar"));

    BOOST_REQUIRE(od2);
    BOOST_REQUIRE_EQUAL(d1, *od2);
}

BOOST_AUTO_TEST_CASE(create_Y_shape) {
    fs::path testdir = choose_test_dir();
    Random random;

    LocalBranch branch1 = create_branch(testdir, "user1_id");
    LocalBranch branch2 = create_branch(testdir, "user2_id");

    BOOST_REQUIRE_EQUAL(branch1.object_directory(), branch2.object_directory());

    auto objdir = branch1.object_directory();

    BOOST_REQUIRE_EQUAL(branch1.root_id(), branch2.root_id());

    BOOST_REQUIRE_EQUAL(count_objects(objdir), 1 /* root */);

    //----------------------------------------------------------------
    branch1.mkdir(Path("A"));
    branch2.mkdir(Path("B"));
    branch1.mkdir(Path("A/C"));

    //          o   o
    //         A|  /
    //         1o /B
    //         C|/
    //         2o

    BOOST_REQUIRE_EQUAL(branch1.id_of(Path("A/C")).hex(),
                        branch2.id_of(Path("B")).hex());

    BOOST_REQUIRE_EQUAL(::refcount(branch1, "A"), 1);
    BOOST_REQUIRE_EQUAL(::refcount(branch2, "B"), 2);
    BOOST_REQUIRE_EQUAL(::refcount(branch1, "A/C"), 2);

    //----------------------------------------------------------------
    branch2.mkdir(Path("B/C"));

    //          o   o
    //          A\ /B
    //           2o
    //            |C
    //           1o

    BOOST_REQUIRE_EQUAL(branch1.id_of(Path("A")).hex(),
                        branch2.id_of(Path("B")).hex());

    BOOST_REQUIRE_EQUAL(::refcount(branch1, "A"), 2);
    BOOST_REQUIRE_EQUAL(::refcount(branch2, "B"), 2);
    BOOST_REQUIRE_EQUAL(::refcount(branch1, "A/C"), 1);
    BOOST_REQUIRE_EQUAL(::refcount(branch2, "B/C"), 1);

    ////----------------------------------------------------------------
    auto data  = random.vector(256);

    branch1.store(Path("A/C/D"), data);
    branch2.store(Path("B/C/D"), data);

    //          o   o
    //          A\ /B
    //            o
    //            |C
    //            o
    //            |D
    //            o

    BOOST_REQUIRE_EQUAL(branch1.id_of(Path("A")).hex(),
                        branch2.id_of(Path("B")).hex());

    BOOST_REQUIRE_EQUAL(::refcount(branch1, "A"), 2);
    BOOST_REQUIRE_EQUAL(::refcount(branch2, "B"), 2);
    BOOST_REQUIRE_EQUAL(::refcount(branch1, "A/C"), 1);
    BOOST_REQUIRE_EQUAL(::refcount(branch1, "A/C/D"), 1);

    //----------------------------------------------------------------
}

BOOST_AUTO_TEST_CASE(tree_remove) {
    fs::path testdir = choose_test_dir();

    Random random;

    // Delete data from root
    {
        auto data = random.blob(256);

        LocalBranch branch = create_branch(testdir/"1", "user_id");
        branch.store("data", data);

        Tree root = object::io::load<Tree>(branch.object_directory(), branch.root_id());

        BOOST_REQUIRE_EQUAL(root.size(), 1);
        BOOST_REQUIRE_EQUAL(root.begin()->first, "data");
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 2);

        Blob blob = object::io::load<Blob>(branch.object_directory(), root.begin()->second);
        BOOST_REQUIRE_EQUAL(data, blob);

        bool removed = branch.remove("data");
        BOOST_REQUIRE(removed);

        root = object::io::load<Tree>(branch.object_directory(), branch.root_id());

        BOOST_REQUIRE_EQUAL(root.size(), 0);
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 1);
    }

    // Delete data from subdir, then delete the subdir
    {
        LocalBranch branch = create_branch(testdir/"2", "user_id");

        auto data = random.vector(256);
        branch.mkdir(Path("dir"));
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
        LocalBranch branch = create_branch(testdir/"3", "user_id");

        auto data = random.vector(256);
        branch.mkdir(Path("dir"));
        branch.store("dir/data", data);

        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 3);

        bool removed = branch.remove("dir");
        BOOST_REQUIRE(removed);

        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 1);
    }

    // Delete data from one root, preserve (using refcount) in the other
    {
        LocalBranch branch1 = create_branch(testdir/"4", "user1_id");
        LocalBranch branch2 = create_branch(testdir/"4", "user2_id");

        BOOST_REQUIRE_EQUAL(branch1.object_directory(), branch2.object_directory());
        auto objdir = branch1.object_directory();

        auto data  = random.vector(256);

        BOOST_REQUIRE_EQUAL(count_objects(objdir), 1 /* root */);

        branch1.store("data", data);
        BOOST_REQUIRE_EQUAL(count_objects(objdir), 2 /* root */ + 1 /* data */);

        branch1.store("other_data", random.vector(256));
        BOOST_REQUIRE_EQUAL(count_objects(objdir), 2 /* root */ + 2 /* data */);

        branch2.store("data", data);
        BOOST_REQUIRE_EQUAL(count_objects(objdir), 2 /* root */ + 2 /* data */);
        branch2.store("other_data", random.vector(256));

        BOOST_REQUIRE_EQUAL(count_objects(objdir), 2 /* roots */ + 3 /* data */);

        branch1.remove("data");

        // Same count as before, since "data" is still in branch2
        BOOST_REQUIRE_EQUAL(count_objects(objdir), 2 /* roots */ + 3 /* data */);

        branch2.remove("data");

        // Now "data" should have been removed all together
        BOOST_REQUIRE_EQUAL(count_objects(objdir), 2 /* roots */ + 2 /* data */);
    }

    {
        LocalBranch branch1 = create_branch(testdir/"5", "user1_id");
        auto objdir = branch1.object_directory();

        BOOST_REQUIRE_EQUAL(count_objects(objdir), 1 /* root */);

        auto data  = random.vector(256);

        branch1.mkdir(Path("A"));
        branch1.mkdir(Path("A/B"));
        branch1.store(Path("A/B/C"), data);

        BOOST_REQUIRE_EQUAL(count_objects(objdir), 4);

        branch1.remove(Path("A/B"));

        BOOST_REQUIRE_EQUAL(count_objects(objdir), 2);
    }

    {
        LocalBranch branch1 = create_branch(testdir/"6", "user1_id");
        LocalBranch branch2 = create_branch(testdir/"6", "user2_id");

        BOOST_REQUIRE_EQUAL(branch1.object_directory(), branch2.object_directory());
        auto objdir = branch1.object_directory();

        BOOST_REQUIRE_EQUAL(count_objects(objdir), 1 /* root */);

        auto data  = random.vector(256);

        //                   o   o
        //                   A\ /B
        //                     o
        //                     |C
        //                     o
        //                     |D
        //                     *

        // BR1 = BR2
        BOOST_REQUIRE_EQUAL(count_objects(objdir), 1);

        branch1.mkdir(Path("A"));
        // (BR1/A = BR2) + BR1
        BOOST_REQUIRE_EQUAL(count_objects(objdir), 2);

        branch2.mkdir(Path("B"));
        // BR1 + BR2 + (BR1/A = BR2/B)
        BOOST_REQUIRE_EQUAL(count_objects(objdir), 3);

        branch1.mkdir(Path("A/C"));
        branch2.mkdir(Path("B/C"));
        branch1.store(Path("A/C/D"), data);
        branch2.store(Path("B/C/D"), data);

        auto refcount = [&] (auto& branch, const fs::path path) {
            auto id = branch.id_of(Path(path));
            return ouisync::refcount::read_recursive(objdir, id);
        };

        BOOST_REQUIRE_EQUAL(refcount(branch1, "A"), 2);
        BOOST_REQUIRE_EQUAL(refcount(branch2, "B"), 2);
        BOOST_REQUIRE_EQUAL(refcount(branch1, "A/C"), 1);
        BOOST_REQUIRE_EQUAL(refcount(branch1, "A/C/D"), 1);

        BOOST_REQUIRE_EQUAL(count_objects(objdir), 5);

        branch1.remove(Path("A/C/D"));

        BOOST_REQUIRE_EQUAL(count_objects(objdir), 7);
    }
}
