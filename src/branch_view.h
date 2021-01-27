#pragma once

#include "object/tree.h"
#include "object/blob.h"
#include "path_range.h"
#include "file_system_attrib.h"

#include <boost/filesystem/path.hpp>

namespace ouisync {

class ObjectStore;

class BranchView {
public:
    using Tree = object::Tree;
    using Blob = object::Blob;

public:
    BranchView(ObjectStore&, const ObjectId& root_id);

    std::set<std::string> readdir(PathRange path) const;

    FileSystemAttrib get_attr(PathRange path) const;

    size_t read(PathRange path, const char* buf, size_t size, size_t offset) const;

    Opt<Blob> maybe_load(PathRange) const;

    ObjectId id_of(PathRange) const;

    void show(std::ostream&) const;

    bool object_exists(const ObjectId&) const;

    friend std::ostream& operator<<(std::ostream& os, const BranchView& b) {
        b.show(os);
        return os;
    }

private:
    ObjectStore& _objects;
    const ObjectId& _root_id;
};

} // namespace
