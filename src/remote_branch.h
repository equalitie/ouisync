#pragma once

#include "object_id.h"
#include "object/tree.h"
#include "object/blob.h"
#include "shortcuts.h"
#include "version_vector.h"
#include "path_range.h"
#include "file_system_attrib.h"
#include "commit.h"
#include "branch_io.h"

#include <boost/asio/awaitable.hpp>
#include <boost/filesystem/path.hpp>
#include <map>

namespace ouisync {

class InputArchive;
class OutputArchive;
class Snapshot;

class RemoteBranch {
private:
    using Tree = object::Tree;
    using Blob = object::Blob;

public:
    RemoteBranch(Commit, fs::path filepath, fs::path objdir);

    static
    RemoteBranch load(fs::path filepath, fs::path objdir);

    [[nodiscard]] net::awaitable<ObjectId> insert_blob(const Blob&);
    [[nodiscard]] net::awaitable<ObjectId> insert_tree(const Tree&);

    [[nodiscard]] net::awaitable<void> introduce_commit(const Commit&);

    void destroy();

    const VersionVector& stamp() const { return _commit.stamp; }
    const ObjectId& root_id() const { return _commit.root_id; }

    BranchIo::Immutable immutable_io() const {
        return BranchIo::Immutable(_objdir, root_id());
    }

    template<class Archive>
    void serialize(Archive& ar, unsigned) {
        ar & _commit
           & _complete_objects
           & _incomplete_objects
           & _missing_objects;
    }

    Snapshot create_snapshot(const fs::path& snapshotdir) const;

private:
    RemoteBranch(fs::path filepath, fs::path objdir);

    void store_tag(OutputArchive&) const;
    void store_body(OutputArchive&) const;
    void load_body(InputArchive&);

    template<class T> void store(const fs::path&, const T&);
    template<class T> void load(const fs::path&, T& value);

    void store_self() const;

    template<class Obj>
    net::awaitable<ObjectId> insert_object(const Obj& obj, std::set<ObjectId> children);

    void filter_missing(std::set<ObjectId>& objs) const;

private:
    fs::path _filepath;
    fs::path _objdir;

    using Parents  = std::set<ObjectId>;
    using Children = std::set<ObjectId>;

    // Objects whose all children have been downloaded and also whose parent's
    // are either non existent (root) or incomplete.
    std::set<ObjectId> _complete_objects;

    // Objects that have been downloaded, but some of its childrent haven't
    // been.
    std::map<ObjectId, Children> _incomplete_objects;

    // Objects that are known to be in this snapshot, but haven't yet been
    // downloaded.
    std::map<ObjectId, Parents> _missing_objects;

    Commit _commit;
};

} // namespace
