#pragma once

#include "version_vector.h"
#include "block_id.h"
#include "ouisync_assert.h"

namespace ouisync {

struct VersionedObject {
    BlockId id;
    VersionVector versions;

    bool happened_before(const VersionedObject& other) const {
        return versions.happened_before(other.versions);
    }

    bool happened_after(const VersionedObject& other) const {
        return versions.happened_after(other.versions);
    }

    bool operator==(const VersionedObject& other) const {
        bool same_version = versions == other.versions;
        if (same_version) ouisync_assert(id == other.id);
        return same_version;
    }

    bool operator<(const VersionedObject& other) const {
        return id < other.id;
    }

    bool has_smaller_user_version(const VersionedObject& other, const UserId& user) const {
        return versions.version_of(user) < other.versions.version_of(user);
    }

    template<class Archive>
    void serialize(Archive& ar, const unsigned int) {
        ar & id;
        ar & versions;
    }

    friend std::ostream& operator<<(std::ostream&, const VersionedObject&);
};

} // namespace
