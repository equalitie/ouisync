#pragma once

#include "path_range.h"
#include "user_id.h"
#include "versioned_object.h"

#include <set>
#include <map>
#include <string>

namespace ouisync {

class BlockStore;

class MultiDir {
public:
    struct Version {
        UserId user;
        VersionedObject vobj;
    };

    using Versions = std::map<UserId, VersionedObject>;

public:
    MultiDir(Versions versions, BlockStore& block_store) :
        _versions(std::move(versions)),
        _block_store(&block_store)
    {}

    MultiDir cd_into(const std::string& where) const;

    MultiDir cd_into(PathRange path) const;

    std::set<std::string> list() const;

    BlockId file(const std::string& name) const;

    Opt<Version> pick_subdirectory_to_edit(const UserId& preferred_user, const string_view name);

private:
    std::map<std::string, BlockId> list_() const;

private:
    Versions _versions;
    BlockStore* _block_store;
};

} // namespace
