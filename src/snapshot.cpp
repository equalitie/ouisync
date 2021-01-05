#include "snapshot.h"
#include "hash.h"
#include "hex.h"
#include "variant.h"
#include "refcount.h"
#include "archive.h"
#include "random.h"
#include "branch_io.h"
#include "object/tree.h"
#include "object/blob.h"
#include "object/io.h"
#include "ouisync_assert.h"

#include <boost/filesystem/fstream.hpp>
#include <boost/serialization/set.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>
#include <boost/serialization/vector.hpp>

#include <iostream>

using namespace ouisync;
using namespace std;
using object::Tree;

static auto _generate_random_name_tag()
{
    Snapshot::NameTag rnd;
    random::generate_non_blocking(rnd.data(), rnd.size());
    return rnd;
}

static auto _path_from_tag(const Snapshot::NameTag& name_tag, const fs::path& dir)
{
    auto hex = to_hex<char>(name_tag);
    return dir / fs::path(hex.begin(), hex.end());
}

ObjectId Snapshot::calculate_id() const
{
    Sha256 hash;
    hash.update("Snapshot");
    hash.update(_commit.root_id);

    hash.update(uint32_t(_objects.complete.size()));
    for (auto& id : _objects.complete) {
        hash.update(id);
    }

    hash.update(uint32_t(_objects.incomplete.size()));
    for (auto& [id, type] : _objects.incomplete) {
        hash.update(id);
    }

    hash.update(uint32_t(_objects.missing.size()));
    for (auto& [id, type] : _objects.missing) {
        hash.update(id);
    }

    return hash.close();
}

Snapshot::Snapshot(fs::path objdir, fs::path snapshotdir, Commit commit) :
    _name_tag(_generate_random_name_tag()),
    _path(_path_from_tag(_name_tag, snapshotdir)),
    _objdir(std::move(objdir)),
    _snapshotdir(std::move(snapshotdir)),
    _commit(move(commit))
{
    _objects.missing.insert({_commit.root_id, {}});
}

Snapshot::Snapshot(Snapshot&& other) :
    _name_tag(other._name_tag),
    _path(std::move(other._path)),
    _objdir(std::move(other._objdir)),
    _snapshotdir(std::move(other._snapshotdir)),
    _commit(move(other._commit)),
    _objects(move(other._objects))
{
}

Snapshot& Snapshot::operator=(Snapshot&& other)
{
    forget();

    _name_tag = other._name_tag;
    _path     = std::move(other._path);
    _objdir   = std::move(other._objdir);
    _commit   = std::move(other._commit);
    _objects  = move(other._objects);

    return *this;
}

/* static */
Snapshot Snapshot::create(Commit commit, Options::Snapshot options)
{
    Snapshot s(std::move(options.objectdir),
            move(options.snapshotdir), std::move(commit));

    s.store();
    return s;
}

void Snapshot::notify_parent_that_child_completed(const ObjectId& parent_id, const ObjectId& child)
{
    auto& parent = _objects.incomplete.at(parent_id);

    parent.missing_children.erase(child);

    if (!parent.missing_children.empty()) {
        return;
    }

    Rc rc = Rc::load(_objdir, parent_id);
    rc.decrement_direct_count();
    rc.increment_recursive_count();

    auto grandparents = move(parent.parents);
    _objects.incomplete.erase(parent_id);
    _objects.complete.insert(parent_id);

    // We no longer need to keep track of it as it's covered by the
    // parent.
    _objects.complete.erase(child);

    for (auto grandparent : grandparents) {
        notify_parent_that_child_completed(grandparent, parent_id);
    }
}

void Snapshot::insert_object(const ObjectId& id, set<ObjectId> children)
{
    // XXX: We should check here whether `children` are already stored on
    // disk and if so, not mark them as missing. We'll also need to update
    // their rc counts appropriately.

    bool is_complete = children.empty();

    if (is_complete) {
        Rc::load(_objdir, id).increment_recursive_count();
    } else {
        Rc::load(_objdir, id).increment_direct_count();
    }

    auto missing_obj_i = _objects.missing.find(id);

    if (missing_obj_i == _objects.missing.end()) {
        throw std::runtime_error(
                "Snapshot: Inserting object that is not known to be missing");
    }

    auto missing_obj = move(missing_obj_i->second);
    _objects.missing.erase(missing_obj_i);

    if (children.empty()) {
        _objects.complete.insert(id);

        // Check that any of the parents of `id` became "complete".
        for (auto& parent_id : missing_obj.parents) {
            notify_parent_that_child_completed(parent_id, id);
        }
    } else {
        for (auto& child : children) {
            _objects.missing[child].parents.insert(id);
        }

        _objects.incomplete.insert({id, {move(missing_obj.parents), move(children)}});
    }
}

void Snapshot::store()
{
    archive::store(_path, _objects);
}

void Snapshot::forget() noexcept
{
    auto objects = move(_objects);

    try {
        for (auto& id : objects.complete) {
            refcount::deep_remove(_objdir, id);
        }
        for (auto& [id, _] : objects.incomplete) {
            refcount::flat_remove(_objdir, id);
        }
    }
    catch (const std::exception& e) {
        exit(1);
    }
}

Snapshot Snapshot::clone() const
{
    Snapshot c(_objdir, _snapshotdir, _commit);

    for (auto& id : _objects.complete) {
        c._objects.complete.insert(id);
        Rc::load(_objdir, id).increment_recursive_count();
    }

    for (auto& o : _objects.incomplete) {
        c._objects.incomplete.insert(o);
        Rc::load(_objdir, o.first).increment_direct_count();
    }

    for (auto& o : _objects.missing) {
        c._objects.missing.insert(o);
    }

    return c;
}

void Snapshot::sanity_check() const
{
    for (auto& [id, _] : _objects.incomplete) {
        ouisync_assert(object::io::exists(_objdir, id));
    }

    for (auto& id : _objects.complete) {
        ouisync_assert(object::io::is_complete(_objdir, id));
    }
}

Snapshot::~Snapshot()
{
    forget();
}

std::ostream& ouisync::operator<<(std::ostream& os, const Snapshot& s)
{
    os << "Snapshot: " << s._commit << "\n";
    os << BranchIo::Immutable(s._objdir, s._commit.root_id);
    os << "Complete objs: ";
    for (auto& id : s._objects.complete) {
        os << id << ", ";
    }
    os << "\nIncomplete objs: ";
    for (auto& [id, _] : s._objects.incomplete) {
        os << id << ", ";
    }
    return os << "\n";
}

////////////////////////////////////////////////////////////////////////////////
// SnapshotGroup

SnapshotGroup::Id SnapshotGroup::calculate_id() const
{
    Sha256 hash;
    hash.update("SnapshotGroup");
    hash.update(uint32_t(size()));
    for (auto& [user_id, snapshot] : *this) {
        hash.update(user_id.to_string());
        hash.update(snapshot.calculate_id());
    }
    return hash.close();
}

SnapshotGroup::~SnapshotGroup()
{
    for (auto& [_, s] : static_cast<Parent&>(*this)) {
        s.forget();
    }
}

std::ostream& ouisync::operator<<(std::ostream& os, const SnapshotGroup& g)
{
    os << "SnapshotGroup{id:" << g.id() << " [";
    bool is_first = true;
    for (auto& [user_id, snapshot] : g) {
        if (!is_first) { os << ", "; }
        is_first = false;
        os << "(" << user_id << ", " << snapshot << ")";
    }
    return os << "]}";
}
