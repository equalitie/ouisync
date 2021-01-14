#define BOOST_TEST_MODULE objects
#include <boost/test/included/unit_test.hpp>

#include "object/blob.h"
#include "object/tree.h"
#include "object/io.h"
#include "refcount.h"
#include "shortcuts.h"
#include "hex.h"
#include "local_branch.h"
#include "path_range.h"
#include "utils.h"
#include "object_store.h"

#include <iostream>
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
    BOOST_REQUIRE_EQUAL(b1.calculate_id(), b2.calculate_id());
}

BOOST_AUTO_TEST_CASE(tree_is_same) {
    fs::path testdir = choose_test_dir();

    Random random;
    object::Tree t1;
    t1[random.string(2)].set_id(random.object_id());
    t1[random.string(10)].set_id(random.object_id());
    object::io::store(testdir, t1);
    auto t2 = object::io::load<object::Tree>(testdir, t1.calculate_id());
    BOOST_REQUIRE_EQUAL(t1.calculate_id(), t2.calculate_id());
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

struct Branch {
    ObjectStore objects;
    LocalBranch branch;

    Branch(fs::path branchdir, UserId user_id, Options::LocalBranch opts) :
        objects(opts.objectdir),
        branch(LocalBranch::create(branchdir, user_id, objects, opts))
    {}

    size_t refcount(const fs::path path) {
        auto id = branch.id_of(path_range(path));
        return objects.rc(id).recursive_count();
    };

    auto object_directory() { return branch.object_directory(); };

    auto root_id() const { return branch.root_id(); }

    void mkdir(PathRange path) { return branch.mkdir(path); }
    bool remove(PathRange path) { return branch.remove(path); }
    bool remove(const fs::path& path) { return branch.remove(path); }
    void store(PathRange path, const Blob& blob) { return branch.store(path, blob); }
    void store(fs::path path, const Blob& blob) { return branch.store(path, blob); }

    BranchIo::Immutable immutable_io() const {
        return branch.immutable_io();
    }

    ObjectId id_of(PathRange path) const { return branch.id_of(path); }
};

Branch create_branch(const fs::path testdir, const char* user_id_file_name) {
    fs::path objdir = testdir/"objects";
    fs::path branchdir = testdir/"branches";
    fs::path snapshotdir = testdir/"snapshots";

    fs::create_directories(objdir);
    fs::create_directories(branchdir);
    fs::create_directories(snapshotdir);

    Options::LocalBranch options{.objectdir = objdir, .snapshotdir = snapshotdir};

    UserId user_id = UserId::load_or_create(testdir/user_id_file_name);

    Random random;
    auto name = to_hex(random.string(16));
    return Branch(branchdir/name, user_id, move(options));
    //return LocalBranch::create(branchdir/name, user_id, move(options));
}

BOOST_AUTO_TEST_CASE(branch_directories) {
    fs::path testdir = choose_test_dir();

    {
        Branch branch = create_branch(testdir/"1", "user_id");
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 1);

        branch.mkdir(Path("dir"));
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 2);
        BOOST_REQUIRE_EQUAL(branch.refcount("dir"), 1);
    }

    {
        Branch branch = create_branch(testdir/"2", "user_id");
        auto empty_root_id = branch.root_id();

        branch.mkdir(Path("dir"));
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 2);
        BOOST_REQUIRE_EQUAL(branch.refcount("dir"), 1);

        branch.remove(Path("dir"));
        BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 1);

        BOOST_REQUIRE_EQUAL(branch.root_id(), empty_root_id);
    }
}

BOOST_AUTO_TEST_CASE(tree_branch_store_and_load) {
    fs::path testdir = choose_test_dir();

    Random random;

    Blob d1(random.vector(1000));

    Branch branch = create_branch(testdir, "user_id");

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

    Branch branch = create_branch(testdir, "user_id");

    branch.mkdir(Path("foo"));
    BOOST_REQUIRE_EQUAL(branch.refcount("foo"), 1);

    branch.store("foo/bar", d1);

    BOOST_REQUIRE_EQUAL(count_objects(branch.object_directory()), 3 /* root + foo + bar */);

    auto od2 = branch.immutable_io().maybe_load(Path("foo/bar"));

    BOOST_REQUIRE(od2);
    BOOST_REQUIRE_EQUAL(d1, *od2);
}

BOOST_AUTO_TEST_CASE(create_Y_shape) {
    fs::path testdir = choose_test_dir();
    Random random;

    Branch branch1 = create_branch(testdir, "user1_id");
    Branch branch2 = create_branch(testdir, "user2_id");

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

    BOOST_REQUIRE_EQUAL(branch1.id_of(Path("A/C")),
                        branch2.id_of(Path("B")));

    BOOST_REQUIRE_EQUAL(branch1.refcount("A"), 1);
    BOOST_REQUIRE_EQUAL(branch2.refcount("B"), 2);
    BOOST_REQUIRE_EQUAL(branch1.refcount("A/C"), 2);

    //----------------------------------------------------------------
    branch2.mkdir(Path("B/C"));

    //          o   o
    //          A\ /B
    //           2o
    //            |C
    //           1o

    BOOST_REQUIRE_EQUAL(branch1.id_of(Path("A")),
                        branch2.id_of(Path("B")));

    BOOST_REQUIRE_EQUAL(branch1.refcount("A"), 2);
    BOOST_REQUIRE_EQUAL(branch2.refcount("B"), 2);
    BOOST_REQUIRE_EQUAL(branch1.refcount("A/C"), 1);
    BOOST_REQUIRE_EQUAL(branch2.refcount("B/C"), 1);

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

    BOOST_REQUIRE_EQUAL(branch1.id_of(Path("A")),
                        branch2.id_of(Path("B")));

    BOOST_REQUIRE_EQUAL(branch1.refcount("A"), 2);
    BOOST_REQUIRE_EQUAL(branch2.refcount("B"), 2);
    BOOST_REQUIRE_EQUAL(branch1.refcount("A/C"), 1);
    BOOST_REQUIRE_EQUAL(branch1.refcount("A/C/D"), 1);

    //----------------------------------------------------------------
}

BOOST_AUTO_TEST_CASE(tree_remove) {
    fs::path testdir = choose_test_dir();

    Random random;

    // Delete data from root
    {
        auto data = random.blob(256);

        Branch branch = create_branch(testdir/"1", "user_id");
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
        Branch branch = create_branch(testdir/"2", "user_id");

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
        Branch branch = create_branch(testdir/"3", "user_id");

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
        Branch branch1 = create_branch(testdir/"4", "user1_id");
        Branch branch2 = create_branch(testdir/"4", "user2_id");

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
        Branch branch1 = create_branch(testdir/"5", "user1_id");
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
        Branch branch1 = create_branch(testdir/"6", "user1_id");
        Branch branch2 = create_branch(testdir/"6", "user2_id");

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

        BOOST_REQUIRE_EQUAL(branch1.refcount("A"), 2);
        BOOST_REQUIRE_EQUAL(branch2.refcount("B"), 2);
        BOOST_REQUIRE_EQUAL(branch1.refcount("A/C"), 1);
        BOOST_REQUIRE_EQUAL(branch1.refcount("A/C/D"), 1);

        BOOST_REQUIRE_EQUAL(count_objects(objdir), 5);

        branch1.remove(Path("A/C/D"));

        BOOST_REQUIRE_EQUAL(count_objects(objdir), 7);
    }
}
