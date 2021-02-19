#pragma once

#include "file_blob.h"
#include "path_range.h"
#include "user_id.h"
#include "commit.h"

#include <set>
#include <map>
#include <string>

namespace ouisync {

class ObjectStore;

class MultiDir {
public:
    using Versions = std::map<UserId, Commit>;

public:
    MultiDir(Versions versions, ObjectStore& objstore) :
        versions(std::move(versions)),
        objstore(&objstore)
    {}

    bool has_subdirectory(string_view) const;

    MultiDir cd_into(const std::string& where) const;

    MultiDir cd_into(PathRange path) const;

    std::map<std::string, ObjectId> list() const;

    ObjectId file(const std::string& name) const;

private:
    Versions versions;
    ObjectStore* objstore;
};

} // namespace
