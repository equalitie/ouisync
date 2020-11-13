#pragma once

#include "version_vector.h"
#include "object/id.h"

namespace ouisync {

struct Commit {
    VersionVector version_vector;
    object::Id root_object_id;

    bool operator<(const Commit& other) const {
        return root_object_id < other.root_object_id;
    }

    template<class Archive>
    void serialize(Archive& ar, const unsigned int version) {
        ar & root_object_id;
        ar & version_vector;
    }
};

} // namespace
