#pragma once

#include "commit.h"
#include "shortcuts.h"

#include <set>
#include <map>
#include <boost/filesystem/path.hpp>
#include <iosfwd>

namespace ouisync {

namespace object { class Tree; struct Blob; }

//////////////////////////////////////////////////////////////////////

class Snapshot {
public:
    using Id = ObjectId;
    using Tree = object::Tree;
    using Blob = object::Blob;
    using Object = variant<Blob, Tree>;

public:
    Snapshot(const Snapshot&) = delete;
    Snapshot& operator=(const Snapshot&) = delete;

    Snapshot(Snapshot&&);
    Snapshot& operator=(Snapshot&&);

    static Snapshot create(const fs::path& snapshotdir, fs::path objdir, Commit);

    const Commit& commit() const { return _commit; }

    void capture_object(const ObjectId&);

    ~Snapshot();

    ObjectId calculate_id() const;

private:
    Snapshot(fs::path path, fs::path objdir, Commit);

    void store();

    void destroy() noexcept;

    friend std::ostream& operator<<(std::ostream&, const Snapshot&);

private:
    fs::path _path;
    fs::path _objdir;
    Commit _commit;
    std::set<ObjectId> _captured_objs;
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
