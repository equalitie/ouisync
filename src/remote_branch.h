#pragma once

#include "object_id.h"
#include "object/tree.h"
#include "object/blob.h"
#include "shortcuts.h"
#include "version_vector.h"
#include "path_range.h"
#include "file_system_attrib.h"
#include "commit.h"
#include "branch_view.h"
#include "options.h"
#include "object_store.h"

#include <boost/asio/awaitable.hpp>
#include <boost/filesystem/path.hpp>
#include <map>

namespace ouisync {

class Snapshot;
class ObjectStore;

class RemoteBranch {
private:
    using Tree = object::Tree;
    using Blob = object::Blob;

public:
    RemoteBranch(Commit, fs::path filepath, ObjectStore&, Options::RemoteBranch);

    static
    RemoteBranch load(fs::path filepath, ObjectStore&, Options::RemoteBranch);

    [[nodiscard]] net::awaitable<ObjectId> insert_blob(const Blob&);
    [[nodiscard]] net::awaitable<ObjectId> insert_tree(const Tree&);

    void introduce_commit(const Commit&);

    const VersionVector& stamp() const { return _commit.stamp; }
    const ObjectId& root_id() const { return _commit.root_id; }

    BranchView branch_view() const {
        return BranchView(_objects, root_id());
    }

    Snapshot create_snapshot() const;

    void sanity_check() const;

    friend std::ostream& operator<<(std::ostream&, const RemoteBranch&);

private:
    RemoteBranch(fs::path filepath, ObjectStore&, Options::RemoteBranch);

    void store_self() const;

    friend class boost::serialization::access;

    template<class Archive>
    void serialize(Archive& ar, unsigned) {
        // XXX Store snapshot file name
        //ar & _commit
        //   & _complete_objects
        //   & _incomplete_objects
        //   & _missing_objects;
    }

private:
    fs::path _filepath;
    ObjectStore& _objects;
    Options::RemoteBranch _options;
    Commit _commit;
    std::unique_ptr<Snapshot> _snapshot;
};

} // namespace
