#define BOOST_TEST_MODULE version-vector
#include <boost/test/included/unit_test.hpp>
#include <iostream>
#include "version_vector.h"

using namespace std;
using namespace ouisync;

BOOST_AUTO_TEST_CASE(test_version_vector_same) {
    UserId u1 = UserId::generate_random();
    UserId u2 = UserId::generate_random();

    {
        VersionVector v1;
        VersionVector v2;

        BOOST_REQUIRE(v1 == v2);

        BOOST_REQUIRE(! v1.happened_before(v2));
        BOOST_REQUIRE(! v2.happened_before(v1));

        BOOST_REQUIRE(! v1.happened_after(v2));
        BOOST_REQUIRE(! v2.happened_after(v1));
    }

    {
        VersionVector v1;
        VersionVector v2;

        v1.set_version(u1, 2);
        v1.set_version(u2, 5);

        v2.set_version(u1, 2);
        v2.set_version(u2, 5);

        BOOST_REQUIRE(v1 == v2);

        BOOST_REQUIRE(! v1.happened_before(v2));
        BOOST_REQUIRE(! v2.happened_before(v1));

        BOOST_REQUIRE(! v1.happened_after(v2));
        BOOST_REQUIRE(! v2.happened_after(v1));
    }
}

BOOST_AUTO_TEST_CASE(test_version_vector_causal) {
    UserId u1 = UserId::generate_random();
    UserId u2 = UserId::generate_random();

    // Run twice, the second time swap u1 and u2 to test both cases
    // when u1 < u2 and u2 < u1.
    for (unsigned i = 0; i < 2; ++i) {
        {
            VersionVector v1;
            VersionVector v2;

            v1.set_version(u1, 1);
            v1.set_version(u2, 2);

            v2.set_version(u1, 2);
            v2.set_version(u2, 2);

            BOOST_REQUIRE(v1 != v2);

            BOOST_REQUIRE(  v1.happened_before(v2));
            BOOST_REQUIRE(! v2.happened_before(v1));

            BOOST_REQUIRE(! v1.happened_after(v2));
            BOOST_REQUIRE(  v2.happened_after(v1));
        }

        {
            VersionVector v1;
            VersionVector v2;

            v1.set_version(u1, 1);

            v2.set_version(u1, 1);
            v2.set_version(u2, 2);

            BOOST_REQUIRE(v1 != v2);

            BOOST_REQUIRE(  v1.happened_before(v2));
            BOOST_REQUIRE(! v2.happened_before(v1));

            BOOST_REQUIRE(! v1.happened_after(v2));
            BOOST_REQUIRE(  v2.happened_after(v1));
        }

        std::swap(u1,u2);
    }
}

BOOST_AUTO_TEST_CASE(test_version_vector_divergent) {
    UserId u1 = UserId::generate_random();
    UserId u2 = UserId::generate_random();

    // Run twice, the second time swap u1 and u2 to test both cases
    // when u1 < u2 and u2 < u1.
    for (unsigned i = 0; i < 2; ++i) {
        {
            VersionVector v1;
            VersionVector v2;

            v1.set_version(u1, 1);
            v1.set_version(u2, 2);

            v2.set_version(u1, 2);
            v2.set_version(u2, 1);

            BOOST_REQUIRE(v1 != v2);

            BOOST_REQUIRE(! v1.happened_before(v2));
            BOOST_REQUIRE(! v2.happened_before(v1));

            BOOST_REQUIRE(! v1.happened_after(v2));
            BOOST_REQUIRE(! v2.happened_after(v1));
        }

        {
            VersionVector v1;
            VersionVector v2;

            v1.set_version(u1, 1);

            v2.set_version(u2, 1);

            BOOST_REQUIRE(v1 != v2);

            BOOST_REQUIRE(! v1.happened_before(v2));
            BOOST_REQUIRE(! v2.happened_before(v1));

            BOOST_REQUIRE(! v1.happened_after(v2));
            BOOST_REQUIRE(! v2.happened_after(v1));
        }

        std::swap(u1, u2);
    }
}

BOOST_AUTO_TEST_CASE(test_version_vector_merge_same) {
    UserId u1 = UserId::generate_random();
    UserId u2 = UserId::generate_random();

    VersionVector v1;
    VersionVector v2;

    v1.set_version(u1, 1);
    v1.set_version(u2, 2);

    v2.set_version(u1, 1);
    v2.set_version(u2, 2);

    VersionVector v3 = v1.merge(v2);

    BOOST_REQUIRE(v3 == v1);
    BOOST_REQUIRE(v3 == v2);

    BOOST_REQUIRE(! v3.happened_after(v1));
    BOOST_REQUIRE(! v3.happened_after(v2));
}

BOOST_AUTO_TEST_CASE(test_version_vector_merge_divergent) {
    UserId u1 = UserId::generate_random();
    UserId u2 = UserId::generate_random();
    UserId u3 = UserId::generate_random();

    VersionVector v1;
    VersionVector v2;

    v1.set_version(u1, 1);
    v1.set_version(u2, 2);

    v2.set_version(u1, 2);
    v2.set_version(u2, 1);
    v2.set_version(u3, 1);

    VersionVector v3 = v1.merge(v2);

    BOOST_REQUIRE(v3 != v1);
    BOOST_REQUIRE(v3 != v2);

    BOOST_REQUIRE(v3.happened_after(v1));
    BOOST_REQUIRE(v3.happened_after(v2));
}
