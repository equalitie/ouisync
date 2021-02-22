#pragma once

#include "directory.h"
#include "file_blob.h"
#include "path_range.h"
#include "file_system_attrib.h"
#include "versioned_object.h"

#include <boost/filesystem/path.hpp>
#include <map>
#include <set>

namespace ouisync {

class ObjectStore;
class MultiDir;

class BranchView {
public:
    BranchView(ObjectStore&, std::map<UserId, VersionedObject> commits);

    size_t read(PathRange path, const char* buf, size_t size, size_t offset) const;

    void show(std::ostream&) const;

    bool object_exists(const ObjectId&) const;

    friend std::ostream& operator<<(std::ostream& os, const BranchView& b) {
        b.show(os);
        return os;
    }

private:
    MultiDir root() const;

private:
    ObjectStore& _objects;
    std::map<UserId, VersionedObject> _commits;
};

} // namespace
