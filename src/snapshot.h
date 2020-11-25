#pragma once

#include "commit.h"
#include "shortcuts.h"

#include <set>
#include <boost/filesystem/path.hpp>
#include <iosfwd>

namespace ouisync {

namespace object { class Tree; struct Blob; }

//////////////////////////////////////////////////////////////////////

class Snapshot {
public:
    using Id = object::Id;
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

    const Id& id() const { return _id; }

    ~Snapshot();

private:
    Snapshot(const Id&, fs::path path, fs::path objdir, Commit);

    static void store_commit(const fs::path&, const Commit&);
    static Commit load_commit(const fs::path&);

    void destroy() noexcept;

    friend std::ostream& operator<<(std::ostream&, const Snapshot&);

private:
    // Is invalid if this is default constructed or moved from.
    bool _is_valid = false;
    Id _id;
    fs::path _path;
    fs::path _objdir;
    Commit _commit;
};

//////////////////////////////////////////////////////////////////////

class SnapshotGroup : private std::vector<Snapshot> {
private:
    using Parent = std::vector<Snapshot>;

public:
    using Id = object::Id;

public:
    SnapshotGroup(std::vector<Snapshot> snapshots) :
        Parent(std::move(snapshots)),
        _id(calculate_id())
    {}

    SnapshotGroup(SnapshotGroup&&) = default;
    SnapshotGroup& operator=(SnapshotGroup&) = default;

    const Id& id() const { return _id; }

    using Parent::size;

    auto begin() const { return Parent::begin(); }
    auto end()   const { return Parent::end();   }

    friend std::ostream& operator<<(std::ostream&, const SnapshotGroup&);

private:
    Id calculate_id() const;

private:
    Id _id;
};

//////////////////////////////////////////////////////////////////////

} // namespace
