#pragma once

#include "commit.h"
#include "shortcuts.h"

#include <set>
#include <boost/filesystem/path.hpp>

namespace ouisync {

namespace object { class Tree; struct Blob; }

class Snapshot {
public:
    using Commits = std::set<Commit>;
    using Id = object::Id;
    using Tree = object::Tree;
    using Blob = object::Blob;
    using Object = variant<Blob, Tree>;

public:
    Snapshot(const Snapshot&) = delete;
    Snapshot& operator=(const Snapshot&) = delete;

    Snapshot(Snapshot&&);
    Snapshot& operator=(Snapshot&&);

    static Snapshot create(const fs::path& snapshotdir, fs::path objdir, Commits);

    const Commits& commits() { return _commits; }

    Id id() { return _id; }

    Object load_object(const Id&);

    ~Snapshot();

private:
    Snapshot(const Id&, fs::path path, fs::path objdir, Commits);

    static void store_commits(const fs::path&, const Commits&);
    static Commits load_commits(const fs::path&);

    void destroy() noexcept;

private:
    bool _is_valid = false;
    Id _id;
    fs::path _path;
    fs::path _objdir;
    Commits _commits;
};

} // namespace
