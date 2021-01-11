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

    bool check_complete_and_notify_parents(const ObjectId&);

    bool update_downwards(const ObjectId& node, const std::set<ObjectId>& children);

    std::set<ObjectId> children_of(const ObjectId& id) const;

    void increment_recursive_count(const ObjectId&) const;
    void decrement_recursive_count(const ObjectId&) const;

    void increment_direct_count(const ObjectId&) const;
    void decrement_direct_count(const ObjectId&) const;

    enum class NodeType {
        Missing, Incomplete, Complete
    };

    bool check_all_complete(const std::set<ObjectId>& nodes);

    struct Node {
        NodeType type;
        std::set<ObjectId> parents;
        std::set<ObjectId> children;

        bool is_root() const { return parents.empty(); }

        template<class Archive>
        void serialize(Archive& ar, const unsigned) {
            ar & type & parents & children;
        }
    };

    friend std::ostream& operator<<(std::ostream&, NodeType);
    friend std::ostream& operator<<(std::ostream&, const Node&);
    friend std::ostream& operator<<(std::ostream&, const Snapshot&);


    void update_child(const ObjectId& child, const ObjectId& parent);

private:
    NameTag _name_tag;
    fs::path _path;
    fs::path _objdir;
    fs::path _snapshotdir;
    Commit _commit;

    // Each node here is:
    //
    // * Held by 1 direct count *by this snapshot* if it is "incomplete".
    // * Held by 1 recursive count *by this snapshot* if it is "complete" but
    //   some of its parents are not complete. It is also held by recursive count
    //   from each of its "complete" parents.
    // 
    // Once a node that has parents in _nodes has all its parents "complete",
    // it is evicted from _nodes and it is further held by recursive counts
    // only from its parents.
    //
    // Complete nodes that don't have parents (roots) are still held by 1 recursive
    // count *by this snapshot*.

    std::map<ObjectId, Node> _nodes;
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

    ~SnapshotGroup();

private:
    ObjectId calculate_id() const;

private:
    ObjectId _id;
};

//////////////////////////////////////////////////////////////////////

} // namespace
