#pragma once

#include "file_blob.h"
#include "path_range.h"

#include <set>
#include <map>
#include <string>

namespace ouisync {

class ObjectStore;

class MultiDir {
public:
    MultiDir(std::set<ObjectId> ids, ObjectStore& objstore) :
        ids(std::move(ids)),
        objstore(&objstore)
    {}

    bool has_subdirectory(string_view) const;

    MultiDir cd_into(const std::string& where) const;

    MultiDir cd_into(PathRange path) const;

    std::map<std::string, ObjectId> list() const;

    ObjectId file(const std::string& name) const;

private:
    std::set<ObjectId> ids;
    ObjectStore* objstore;
};

} // namespace
