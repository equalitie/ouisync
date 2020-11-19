#pragma once

#include "object/id.h"
#include "object/tree.h"
#include "object/blob.h"
#include "shortcuts.h"
#include "version_vector.h"
#include "path_range.h"
#include "file_system_attrib.h"
#include "commit.h"

#include <boost/asio/awaitable.hpp>
#include <boost/filesystem/path.hpp>
#include <set>
#include <map>

namespace boost::archive {
    class binary_iarchive;
    class binary_oarchive;
}

namespace ouisync {

class RemoteBranch {
private:
    // Complete objects are those whose all sub-object have also
    // been downloaded. Thus deleting them will delete it's children
    // as well.
    enum class ObjectState { Incomplete, Complete };

    using Id   = object::Id;
    using Tree = object::Tree;
    using Blob = object::Blob;

    struct ObjectEntry {
        ObjectState state;
        std::set<Id> children;

        template<class Archive>
        void serialize(Archive& ar, unsigned version) {
            ar & state & children;
        }
    };

    using IArchive = boost::archive::binary_iarchive;
    using OArchive = boost::archive::binary_oarchive;

public:
    RemoteBranch(Commit, fs::path filepath, fs::path objdir);

    RemoteBranch(fs::path filepath, fs::path objdir, IArchive&);

    [[nodiscard]] net::awaitable<void> mark_complete(const Id&);
    [[nodiscard]] net::awaitable<void> insert_blob(const Blob&);
    [[nodiscard]] net::awaitable<Id>   insert_tree(const Tree&);

    void erase(const fs::path& filepath);

    const VersionVector& version_vector() const { return _version_vector; }
    const object::Id& root_object_id() const { return _root; }

    object::Tree readdir(PathRange) const;
    FileSystemAttrib get_attr(PathRange) const;
    size_t read(PathRange, const char* buf, size_t size, size_t offset) const;


    void store_tag(OArchive&) const;
    void store_rest(OArchive&) const;
    void load_rest(IArchive&);

private:
    template<class T> void store(const fs::path&, const T&);
    template<class T> void load(const fs::path&, T& value);

    [[nodiscard]] net::awaitable<void> insert_object(const Id& objid, std::set<Id> children);

    void store_self() const;

private:
    fs::path _filepath;
    fs::path _objdir;

    Id _root;
    VersionVector _version_vector;

    std::map<object::Id, ObjectEntry> _objects;
};

} // namespace
