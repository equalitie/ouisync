#pragma once

#include "object/id.h"
#include "user_id.h"
#include "version_vector.h"
#include "namespaces.h"

#include <boost/filesystem/path.hpp>

namespace ouisync {

class Branch {
public:
    static Branch load_or_create(
            const fs::path& branchdir, const fs::path& objdir, UserId);

    void store();

    const object::Id& root_object_id() const {
        return _root_id;
    }

    void root_object_id(const object::Id& id);

private:
    Branch(const fs::path& file_path, const UserId& user_id,
            const object::Id& root_id, VersionVector clock);

private:
    fs::path _file_path;
    UserId _user_id;
    object::Id _root_id;
    VersionVector _clock;
};

} // namespace
