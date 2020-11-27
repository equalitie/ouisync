#pragma once

#include "object/id.h"
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
#include <set>
#include <map>

namespace boost::archive {
    class binary_iarchive;
    class binary_oarchive;
}

namespace ouisync {

class RemoteBranch {
private:
    using Id   = object::Id;
    using Tree = object::Tree;
    using Blob = object::Blob;

    using IArchive = boost::archive::binary_iarchive;
    using OArchive = boost::archive::binary_oarchive;

public:
    RemoteBranch(Commit, fs::path filepath, fs::path objdir);

    RemoteBranch(fs::path filepath, fs::path objdir, IArchive&);

    [[nodiscard]] net::awaitable<void> insert_blob(const Blob&);
    [[nodiscard]] net::awaitable<Id>   insert_tree(const Tree&);

    [[nodiscard]] net::awaitable<void> introduce_commit(const Commit&);

    void destroy();

    const VersionVector& stamp() const { return _commit.stamp; }
    const object::Id& root_id() const { return _commit.root_id; }

    BranchIo::Immutable immutable_io() const {
        return BranchIo::Immutable(_objdir, root_id());
    }

private:
    void store_tag(OArchive&) const;
    void store_rest(OArchive&) const;
    void load_rest(IArchive&);

    template<class T> void store(const fs::path&, const T&);
    template<class T> void load(const fs::path&, T& value);

    void store_self() const;

private:
    fs::path _filepath;
    fs::path _objdir;

    Commit _commit;
};

} // namespace
