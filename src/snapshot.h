#pragma once

#include "commit.h"
#include "shortcuts.h"
#include "options.h"

#include <set>
#include <map>
#include <boost/filesystem/path.hpp>
#include <iosfwd>

namespace ouisync {

//////////////////////////////////////////////////////////////////////

class Snapshot {
public:
    using Id = ObjectId;

    // Hex of this type is used to create the file name where Snapshot is
    // stored. Currently Snapshots are not treated as immutable objects but
    // that may change in the future.
    using NameTag = std::array<unsigned char, 16>;

public:
    Snapshot(const Snapshot&) = delete;
    Snapshot& operator=(const Snapshot&) = delete;

    Snapshot(Snapshot&&);
    Snapshot& operator=(Snapshot&&);

    static Snapshot create(Commit, Options::Snapshot);

    const Commit& commit() const { return _commit; }

    void insert_object(const ObjectId&, std::set<ObjectId> children);

    ~Snapshot();

    ObjectId calculate_id() const;

    void forget() noexcept;

    Snapshot clone() const;

    void sanity_check() const;

    NameTag name_tag() const { return _name_tag; }

private:
    Snapshot(fs::path objdir, fs::path snapshotdir, Commit);

    void store();

    friend std::ostream& operator<<(std::ostream&, const Snapshot&);

    void filter_missing(std::set<ObjectId>& objs) const;

private:
    NameTag _name_tag;
    fs::path _path;
    fs::path _objdir;
    fs::path _snapshotdir;
    Commit _commit;

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
};

//////////////////////////////////////////////////////////////////////

class SnapshotGroup : private std::map<UserId, Snapshot> {
private:
    using Parent = std::map<UserId, Snapshot>;

public:
    using Id = ObjectId;

public:
    SnapshotGroup(Parent snapshots) :
        Parent(std::move(snapshots)),
        _id(calculate_id())
    {}

    SnapshotGroup(SnapshotGroup&&) = default;
    SnapshotGroup& operator=(SnapshotGroup&) = default;

    const ObjectId& id() const { return _id; }

    using Parent::size;

    auto begin() const { return Parent::begin(); }
    auto end()   const { return Parent::end();   }

    friend std::ostream& operator<<(std::ostream&, const SnapshotGroup&);

private:
    ObjectId calculate_id() const;

private:
    ObjectId _id;
};

//////////////////////////////////////////////////////////////////////

} // namespace
