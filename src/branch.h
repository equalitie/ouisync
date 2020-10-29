#pragma once

#include "object/id.h"
#include "user_id.h"
#include "version_vector.h"
#include "shortcuts.h"

#include <boost/filesystem/path.hpp>
#include <boost/optional.hpp>

namespace ouisync {

class Branch {
public:
    using Data = std::vector<uint8_t>;

public:
    static Branch load_or_create(
            const fs::path& branchdir, const fs::path& objdir, UserId);

    const object::Id& root_object_id() const {
        return _root_id;
    }

    void root_object_id(const object::Id& id);

    bool maybe_store(const fs::path&, const Data&);

    void store(const fs::path&, const Data&);

    Opt<Data> maybe_load(const fs::path&) const;

    bool remove(const fs::path&);

    const fs::path& object_directory() const {
        return _objdir;
    }

private:
    Branch(const fs::path& file_path, const fs::path& objdir,
            const UserId& user_id, const object::Id& root_id,
            VersionVector clock);

    void store_self() const;

private:
    fs::path _file_path;
    fs::path _objdir;
    UserId _user_id;
    object::Id _root_id;
    VersionVector _clock;
};

} // namespace
