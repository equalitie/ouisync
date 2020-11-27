#pragma once

#include "version_vector.h"
#include "object/id.h"

namespace ouisync {

struct Commit {
    VersionVector stamp;
    object::Id root_id;

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
