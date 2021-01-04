#define BOOST_TEST_MODULE snapshot
#include <boost/test/included/unit_test.hpp>

#include "object/blob.h"
#include "object/tree.h"
#include "object/io.h"
#include "refcount.h"
#include "hex.h"
#include "array_io.h"
#include "local_branch.h"
#include "path_range.h"
#include "utils.h"
#include "snapshot.h"

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

Options::Snapshot create_options(string testname)
{
    auto dir = main_test_dir.subdir(testname);

    Options::Snapshot opts {
        .objectdir   = dir / "objects",
        .snapshotdir = dir / "snapshots"
    };

    fs::create_directories(opts.objectdir);
    fs::create_directories(opts.snapshotdir);

    return opts;
}

struct Environment {
    Environment(string testname)
        : options(create_options(testname))
    {}

    template<class Obj> ObjectId store(const Obj& obj) {
        return object::io::store(options.objectdir, obj);
    }

    Snapshot create_snapshot(ObjectId root)
    {
        return Snapshot::create({{}, root}, options);
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
        return files_in(options.objectdir);
    }

    auto object_files() {
        return files_in(options.objectdir) |
            boost::adaptors::filtered([](auto p) { return !is_refcount(p.path()); });
    }

    Rc load_rc(const ObjectId& id) const {
        return Rc::load(options.objectdir, id);
    }

    Options::Snapshot options;
};

BOOST_AUTO_TEST_CASE(simple_forget) {
    Environment env("simple_forget");

    //           R
    //           ↓
    //           A

    auto A = rnd.blob(256);
    Tree R;
    R["A"].set_id(A.calculate_id());

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

