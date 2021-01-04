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

    hash.update(uint32_t(_complete_objects.size()));
    for (auto& id : _complete_objects) {
        hash.update(id);
    }

    hash.update(uint32_t(_incomplete_objects.size()));
    for (auto& [id, type] : _incomplete_objects) {
        hash.update(id);
    }

    hash.update(uint32_t(_missing_objects.size()));
    for (auto& [id, type] : _missing_objects) {
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
    _missing_objects.insert({_commit.root_id, {}});
}

Snapshot::Snapshot(Snapshot&& other) :
    _name_tag(other._name_tag),
    _path(std::move(other._path)),
    _objdir(std::move(other._objdir)),
    _snapshotdir(std::move(other._snapshotdir)),
    _commit(move(other._commit)),
    _complete_objects(move(other._complete_objects)),
    _incomplete_objects(move(other._incomplete_objects)),
    _missing_objects(move(other._missing_objects))
{
}

Snapshot& Snapshot::operator=(Snapshot&& other)
{
    forget();

    _name_tag = other._name_tag;
    _path     = std::move(other._path);
    _objdir   = std::move(other._objdir);
    _commit   = std::move(other._commit);

    _complete_objects   = move(other._complete_objects);
    _incomplete_objects = move(other._incomplete_objects);
    _missing_objects    = move(other._missing_objects);

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

void Snapshot::filter_missing(set<ObjectId>& objs) const
{
    for (auto i = objs.begin(); i != objs.end();) {
        auto j = std::next(i);
        if (object::io::exists(_objdir, *i)) objs.erase(i);
        i = j;
    }
}

void Snapshot::notify_parent_that_child_completed(const ObjectId& parent_id, const ObjectId& child)
{
    auto& parent = _incomplete_objects.at(parent_id);

    parent.missing_children.erase(child);

    if (!parent.missing_children.empty()) {
        return;
    }

    Rc rc = Rc::load(_objdir, parent_id);
    rc.decrement_direct_count();
    rc.increment_recursive_count();

    auto grandparents = move(parent.parents);
    _incomplete_objects.erase(parent_id);
    _complete_objects.insert(parent_id);

    // We no longer need to keep track of it as it's covered by the
    // parent.
    _complete_objects.erase(child);

    for (auto grandparent : grandparents) {
        notify_parent_that_child_completed(grandparent, parent_id);
    }
}

void Snapshot::insert_object(const ObjectId& id, set<ObjectId> children)
{
    // Missing objects:    Object -> Parents
    // Incomplete objects: Object -> Children

    filter_missing(children);

    bool is_complete = children.empty();

    if (is_complete) {
        Rc::load(_objdir, id).increment_recursive_count();
    } else {
        Rc::load(_objdir, id).increment_direct_count();
    }

    auto missing_obj = move(_missing_objects.at(id));
    _missing_objects.erase(id);

    if (children.empty()) {
        _complete_objects.insert(id);

        // Check that any of the parents of `id` became "complete".
        for (auto& parent_id : missing_obj.parents) {
            notify_parent_that_child_completed(parent_id, id);
        }
    } else {
        for (auto& child : children) {
            _missing_objects[child].parents.insert(id);
        }

        if (children.empty()) {
            _complete_objects.insert(id);
        } else {
            _incomplete_objects.insert({id, {move(missing_obj.parents), move(children)}});
        }
    }
}

void Snapshot::store()
{
    archive::store(_path,
            _complete_objects,
            _incomplete_objects,
            _missing_objects);
}

void Snapshot::forget() noexcept
{
    auto complete_objects   = move(_complete_objects);
    auto incomplete_objects = move(_incomplete_objects);
    _missing_objects.clear();

    try {
        for (auto& id : complete_objects) {
            refcount::deep_remove(_objdir, id);
        }
        for (auto& [id, _] : incomplete_objects) {
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

    for (auto& id : _complete_objects) {
        c._complete_objects.insert(id);
        Rc::load(_objdir, id).increment_recursive_count();
    }

    for (auto& o : _incomplete_objects) {
        c._incomplete_objects.insert(o);
        Rc::load(_objdir, o.first).increment_direct_count();
    }

    for (auto& o : _missing_objects) {
        c._missing_objects.insert(o);
    }

    return c;
}

void Snapshot::sanity_check() const
{
    for (auto& [id, _] : _incomplete_objects) {
        ouisync_assert(object::io::exists(_objdir, id));
    }

    for (auto& id : _complete_objects) {
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
    for (auto& id : s._complete_objects) {
        os << id << ", ";
    }
    os << "\nIncomplete objs: ";
    for (auto& [id, _] : s._incomplete_objects) {
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
