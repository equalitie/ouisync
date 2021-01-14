#define BOOST_TEST_MODULE snapshot
#include <boost/test/included/unit_test.hpp>

#include "object/blob.h"
#include "object/tree.h"
#include "object/io.h"
#include "refcount.h"
#include "local_branch.h"
#include "path_range.h"
#include "utils.h"
#include "snapshot.h"
#include "ostream/set.h"

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

MainTestDir main_test_dir("snapshot");
Random rnd;

struct Directories {
    fs::path objectdir;
    fs::path snapshotdir;
};

Directories create_directories(string testname)
{
    auto dir = main_test_dir.subdir(testname);

    Directories dirs {
        .objectdir   = dir / "objects",
        .snapshotdir = dir / "snapshots"
    };

    fs::create_directories(dirs.objectdir);
    fs::create_directories(dirs.snapshotdir);

    return dirs;
}

struct Environment {
    Environment(string testname)
        : dirs(create_directories(testname))
        , objstore(dirs.objectdir)
    {}

    template<class Obj> ObjectId store(const Obj& obj) {
        return objstore.store(obj);
    }

    Snapshot create_snapshot(ObjectId root)
    {
        return Snapshot::create({{}, root}, objstore, {dirs.snapshotdir});
    }

    static
    bool is_refcount(const fs::path& path) {
        return path.extension() == ".rc";
    }

    static
    auto files_in(const fs::path& path) {
        return fs::recursive_directory_iterator(path) |
            boost::adaptors::filtered([](auto p) {
                    return fs::is_regular_file(p); });
    }

    auto object_dir_files() {
        return files_in(dirs.objectdir);
    }

    auto object_files() {
        return files_in(dirs.objectdir) |
            boost::adaptors::filtered([](auto p) { return !is_refcount(p.path()); });
    }

    Rc load_rc(const ObjectId& id) {
        return objstore.rc(id);
    }

    Directories dirs;
    ObjectStore objstore;
};

BOOST_AUTO_TEST_CASE(simple_forget) {
    Environment env("simple_forget");

    //           R
    //           ↓
    //           A

    auto A = rnd.blob(256);
    Tree R;
    R["A"].set_id(A.calculate_id());

    [[maybe_unused]] auto names =
        { ObjectId::debug_name(R.calculate_id(), "R"),
          ObjectId::debug_name(A.calculate_id(), "A")};

    env.store(R);
    auto snapshot = env.create_snapshot(R.calculate_id());
    snapshot.insert_object(R.calculate_id(), R.children());

    env.store(A);
    snapshot.insert_object(A.calculate_id(), {});

    snapshot.forget();

    BOOST_REQUIRE_EQUAL(boost::distance(env.object_dir_files()), 0);
}

BOOST_AUTO_TEST_CASE(partial_forget) {
    Environment env("partial_forget");

    //           R
    //          ↙ ↘
    //         A   N
    //             ↓
    //             B

    auto A = rnd.blob(256);
    auto B = rnd.blob(256);
    Tree R;
    Tree N;

    N["B"].set_id(B.calculate_id());

    R["A"].set_id(A.calculate_id());
    R["N"].set_id(N.calculate_id());

    auto snapshot = env.create_snapshot(R.calculate_id());

    env.store(R);
    snapshot.insert_object(R.calculate_id(), R.children());

    env.store(A);
    snapshot.insert_object(A.calculate_id(), {});

    env.store(N);
    snapshot.insert_object(N.calculate_id(), N.children());

    snapshot.sanity_check();
    snapshot.forget();

    BOOST_REQUIRE_EQUAL(boost::distance(env.object_dir_files()), 0);
}

BOOST_AUTO_TEST_CASE(two_levels) {
    Environment env("two_levels");

    //           R
    //           ↓
    //           N
    //           ↓
    //           A

    Tree R;
    Tree N;
    Blob A = rnd.blob(256);

    N["A"].set_id(A.calculate_id());
    R["N"].set_id(N.calculate_id());

    auto snapshot = env.create_snapshot(R.calculate_id());

    env.store(R);
    snapshot.insert_object(R.calculate_id(), R.children());

    env.store(N);
    snapshot.insert_object(N.calculate_id(), N.children());

    env.store(A);
    snapshot.insert_object(A.calculate_id(), {});

    auto R_rc = env.load_rc(R.calculate_id());

    BOOST_REQUIRE_EQUAL(R_rc.direct_count()   , 0);
    BOOST_REQUIRE_EQUAL(R_rc.recursive_count(), 1);

    snapshot.sanity_check();
    snapshot.forget();

    BOOST_REQUIRE_EQUAL(boost::distance(env.object_dir_files()), 0);
}

BOOST_AUTO_TEST_CASE(insert_outside_node) {
    Environment env("insert_outside_node");

    // Node A is stored before R is inserted into the snapshot
    //
    //           R
    //           ↓
    //           A

    Tree R;
    Blob A = rnd.blob(256);

    R["A"].set_id(A.calculate_id());

    auto snapshot = env.create_snapshot(R.calculate_id());

    env.store(R);
    env.store(A);
    snapshot.insert_object(R.calculate_id(), R.children());
    snapshot.insert_object(A.calculate_id(), {});

    auto R_rc = env.load_rc(R.calculate_id());
    auto A_rc = env.load_rc(A.calculate_id());

    BOOST_REQUIRE_EQUAL(R_rc.direct_count()   , 0);
    BOOST_REQUIRE_EQUAL(R_rc.recursive_count(), 1);
    BOOST_REQUIRE_EQUAL(A_rc.direct_count()   , 0);
    BOOST_REQUIRE_EQUAL(A_rc.recursive_count(), 1);

    snapshot.sanity_check();
    snapshot.forget();

    BOOST_REQUIRE_EQUAL(boost::distance(env.object_dir_files()), 0);
}

BOOST_AUTO_TEST_CASE(store_then_insert) {
    Environment env("store_then_insert");

    // Nodes are first stored and then inserted into the snapshot
    //
    //           R
    //           ↓
    //           N
    //           ↓
    //           A

    Tree R;
    Tree N;
    Blob A = rnd.blob(256);

    N["A"].set_id(A.calculate_id());
    R["N"].set_id(N.calculate_id());

    auto snapshot = env.create_snapshot(R.calculate_id());

    env.store(R);
    env.store(N);
    env.store(A);
    snapshot.insert_object(R.calculate_id(), R.children());
    snapshot.insert_object(N.calculate_id(), N.children());
    snapshot.insert_object(A.calculate_id(), {});

    auto R_rc = env.load_rc(R.calculate_id());
    auto A_rc = env.load_rc(A.calculate_id());

    BOOST_REQUIRE_EQUAL(R_rc.direct_count()   , 0);
    BOOST_REQUIRE_EQUAL(R_rc.recursive_count(), 1);
    BOOST_REQUIRE_EQUAL(A_rc.direct_count()   , 0);
    BOOST_REQUIRE_EQUAL(A_rc.recursive_count(), 1);

    snapshot.sanity_check();
    snapshot.forget();

    BOOST_REQUIRE_EQUAL(boost::distance(env.object_dir_files()), 0);
}

BOOST_AUTO_TEST_CASE(diamond_shape) {
    Environment env("diamond_shape");

    //
    //           R
    //          ↙ ↘
    //         N   M
    //        ↙ ↘ ↙
    //       B   A     // B is here so that N != M

    Tree R;
    Tree N;
    Tree M;
    Blob A = rnd.blob(256);
    Blob B = rnd.blob(256);

    N["A"].set_id(A.calculate_id());
    N["B"].set_id(B.calculate_id());

    M["A"].set_id(A.calculate_id());

    R["N"].set_id(N.calculate_id());
    R["M"].set_id(M.calculate_id());

    [[maybe_unused]] auto names =
        { ObjectId::debug_name(R.calculate_id(), "R"),
          ObjectId::debug_name(M.calculate_id(), "M"),
          ObjectId::debug_name(N.calculate_id(), "N"),
          ObjectId::debug_name(A.calculate_id(), "A"),
          ObjectId::debug_name(B.calculate_id(), "B")};

    auto snapshot = env.create_snapshot(R.calculate_id());

    env.store(R);
    snapshot.insert_object(R.calculate_id(), R.children());

    env.store(N);
    snapshot.insert_object(N.calculate_id(), N.children());

    env.store(M);
    snapshot.insert_object(M.calculate_id(), M.children());

    env.store(A);
    snapshot.insert_object(A.calculate_id(), {});

    env.store(B);
    snapshot.insert_object(B.calculate_id(), {});

    auto R_rc = env.load_rc(R.calculate_id());
    auto M_rc = env.load_rc(M.calculate_id());
    auto N_rc = env.load_rc(N.calculate_id());
    auto A_rc = env.load_rc(A.calculate_id());
    auto B_rc = env.load_rc(B.calculate_id());

    BOOST_REQUIRE_EQUAL(R_rc.direct_count()   , 0);
    BOOST_REQUIRE_EQUAL(R_rc.recursive_count(), 1);
    BOOST_REQUIRE_EQUAL(N_rc.direct_count()   , 0);
    BOOST_REQUIRE_EQUAL(N_rc.recursive_count(), 1);
    BOOST_REQUIRE_EQUAL(M_rc.direct_count()   , 0);
    BOOST_REQUIRE_EQUAL(M_rc.recursive_count(), 1);
    BOOST_REQUIRE_EQUAL(A_rc.direct_count()   , 0);
    BOOST_REQUIRE_EQUAL(A_rc.recursive_count(), 2);
    BOOST_REQUIRE_EQUAL(B_rc.direct_count()   , 0);
    BOOST_REQUIRE_EQUAL(B_rc.recursive_count(), 1);

    BOOST_REQUIRE_EQUAL(R_rc.recursive_count(), 1);

    snapshot.sanity_check();
    snapshot.forget();

    BOOST_REQUIRE_EQUAL(boost::distance(env.object_dir_files()), 0);
}

BOOST_AUTO_TEST_CASE(auto_insert_existing) {
    Environment env("auto_insert_existing");

    // Nodes are first stored and then inserted into the snapshot
    //
    //           R
    //           ↓
    //           N
    //           ↓
    //           A

    Tree R;
    Tree N;
    Blob A = rnd.blob(256);

    N["A"].set_id(A.calculate_id());
    R["N"].set_id(N.calculate_id());

    [[maybe_unused]] auto names =
        { ObjectId::debug_name(R.calculate_id(), "R"),
          ObjectId::debug_name(N.calculate_id(), "N"),
          ObjectId::debug_name(A.calculate_id(), "A")};

    auto snapshot = env.create_snapshot(R.calculate_id());

    env.store(R);
    env.store(N);
    env.store(A);
    snapshot.insert_object(R.calculate_id(), R.children());

    auto R_rc = env.load_rc(R.calculate_id());
    auto A_rc = env.load_rc(A.calculate_id());

    BOOST_REQUIRE_EQUAL(R_rc.recursive_count(), 1);

    snapshot.sanity_check();
    snapshot.forget();

    BOOST_REQUIRE_EQUAL(boost::distance(env.object_dir_files()), 0);
}

