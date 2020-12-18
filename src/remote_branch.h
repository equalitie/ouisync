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
#include "options.h"

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
    RemoteBranch(Commit, fs::path filepath, Options::RemoteBranch);

    static
    RemoteBranch load(fs::path filepath, Options::RemoteBranch);

    [[nodiscard]] net::awaitable<ObjectId> insert_blob(const Blob&);
    [[nodiscard]] net::awaitable<ObjectId> insert_tree(const Tree&);

    [[nodiscard]] net::awaitable<void> introduce_commit(const Commit&);

    const VersionVector& stamp() const { return _commit.stamp; }
    const ObjectId& root_id() const { return _commit.root_id; }

    BranchIo::Immutable immutable_io() const {
        return BranchIo::Immutable(_options.objectdir, root_id());
    }

    Snapshot create_snapshot() const;

    void sanity_check() const;

    friend std::ostream& operator<<(std::ostream&, const RemoteBranch&);

private:
    RemoteBranch(fs::path filepath, Options::RemoteBranch);

    void store_self() const;

    template<class Obj>
    net::awaitable<ObjectId> insert_object(const Obj& obj, std::set<ObjectId> children);

    void filter_missing(std::set<ObjectId>& objs) const;

    friend class boost::serialization::access;

    template<class Archive>
    void serialize(Archive& ar, unsigned) {
        ar & _commit
           & _complete_objects
           & _incomplete_objects
           & _missing_objects;
    }

    template<class Obj> ObjectId flat_store(const Obj&);
    template<class Obj> ObjectId full_store(const Obj&);

private:
    fs::path _filepath;
    Options::RemoteBranch _options;

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
