#include "remote_branch.h"
#include "branch_type.h"
#include "object/io.h"
#include "refcount.h"
#include "archive.h"
#include "snapshot.h"
#include "ouisync_assert.h"

#include <boost/serialization/vector.hpp>
#include <boost/serialization/set.hpp>

#include <iostream>

using namespace ouisync;

using namespace std;
using object::Tree;

/* static */
RemoteBranch RemoteBranch::load(fs::path filepath, ObjectStore& objects, Options::RemoteBranch options)
{
    RemoteBranch branch(filepath, objects, move(options));
    archive::load(filepath, branch);
    return branch;
}

RemoteBranch::RemoteBranch(Commit commit, fs::path filepath, ObjectStore& objects, Options::RemoteBranch options) :
    _filepath(std::move(filepath)),
    _objects(objects),
    _options(move(options)),
    _commit(move(commit)),
    _snapshot(make_unique<Snapshot>(Snapshot::create(_commit, _objects, _options)))
{
}

RemoteBranch::RemoteBranch(fs::path filepath, ObjectStore& objects, Options::RemoteBranch options) :
    _filepath(std::move(filepath)),
    _objects(objects),
    _options(move(options))
{}

net::awaitable<ObjectId> RemoteBranch::insert_blob(const Blob& blob)
{
    auto id = _objects.store(blob);
    if (!_snapshot) _snapshot = make_unique<Snapshot>(Snapshot::create(_commit, _objects, _options));
    _snapshot->insert_object(id, {});
    store_self();
    co_return id;
}

net::awaitable<ObjectId> RemoteBranch::insert_tree(const Tree& tree)
{
    auto id = _objects.store(tree);
    if (!_snapshot) _snapshot = make_unique<Snapshot>(Snapshot::create(_commit, _objects, _options));
    _snapshot->insert_object(id, tree.children());
    store_self();
    co_return id;
}

void RemoteBranch::introduce_commit(const Commit& commit)
{
    _commit = commit;
    if (_snapshot) { _snapshot->forget(); }
    _snapshot = make_unique<Snapshot>(Snapshot::create(_commit, _objects, _options));
    store_self();
}

//--------------------------------------------------------------------
void RemoteBranch::sanity_check() const {
    ouisync_assert(_snapshot);
    _snapshot->sanity_check();
}

//--------------------------------------------------------------------

Snapshot RemoteBranch::create_snapshot() const
{
    ouisync_assert(_snapshot);
    if (!_snapshot) {
        return Snapshot::create(_commit, _objects, _options);
    }
    return _snapshot->clone();
}

//--------------------------------------------------------------------

void RemoteBranch::store_self() const {
    archive::store(_filepath, *this);
}

//--------------------------------------------------------------------

ostream& ouisync::operator<<(ostream& os, const RemoteBranch& b)
{
    if (!b._snapshot) {
        return os << "RemoteBranch: null";
    } else {
        return os << "RemoteBranch:\n" << *b._snapshot;
    }
}
