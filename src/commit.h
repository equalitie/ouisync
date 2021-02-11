#pragma once

#include "version_vector.h"
#include "object_id.h"
#include "ouisync_assert.h"

namespace ouisync {

struct Commit {
    VersionVector stamp;
    ObjectId root_id;

    bool happened_before(const Commit& other) const {
        return stamp.happened_before(other.stamp);
    }

    bool happened_after(const Commit& other) const {
        return stamp.happened_after(other.stamp);
    }

    bool operator==(const Commit& other) const {
        bool same_stamp = stamp == other.stamp;
        if (same_stamp) ouisync_assert(root_id == other.root_id);
        return same_stamp;
    }

    bool operator<(const Commit& other) const {
        return root_id < other.root_id;
    }

    template<class Archive>
    void serialize(Archive& ar, const unsigned int version) {
        ar & root_id;
        ar & stamp;
    }

    friend std::ostream& operator<<(std::ostream&, const Commit&);
};

} // namespace
