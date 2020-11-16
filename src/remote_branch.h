#pragma once

#include "object/id.h"
#include "object/tree.h"
#include "shortcuts.h"
#include "version_vector.h"
#include "path_range.h"
#include "file_system_attrib.h"

#include <boost/asio/awaitable.hpp>
#include <boost/filesystem/path.hpp>
#include <set>

namespace ouisync {

class RemoteBranch {
public:
    //RemoteBranch(fs::path filepath, fs::path objdir);

    [[nodiscard]] net::awaitable<void> add_complete(const object::Id&);
    [[nodiscard]] net::awaitable<void> add_incomplete(const object::Id&);

    void erase(const fs::path& filepath);

    const VersionVector& version_vector() const { return _version_vector; }
    const object::Id& root_object_id() const { return _root; }

    object::Tree readdir(PathRange) const;
    FileSystemAttrib get_attr(PathRange) const;
    size_t read(PathRange, const char* buf, size_t size, size_t offset) const;

private:
    template<class T> void store(const fs::path&, const T&);
    template<class T> void load(const fs::path&, T& value);

private:
    fs::path _filepath;
    fs::path _objdir;

    VersionVector _version_vector;
    object::Id _root;

    // Complete objects are those whose all sub-object have also
    // been downloaded. Thus deleting them will delete it's children
    // as well.
    std::set<object::Id> _complete_objects;
    std::set<object::Id> _incomplete_objects;
};

} // namespace
