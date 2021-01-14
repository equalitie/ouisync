#include "snapshot.h"
#include "hash.h"
#include "hex.h"
#include "variant.h"
#include "refcount.h"
#include "archive.h"
#include "random.h"
#include "branch_view.h"
#include "object/tree.h"
#include "object/blob.h"
#include "object/io.h"
#include "ouisync_assert.h"
#include "object_store.h"
#include "ostream/set.h"
#include "ostream/map.h"

#include <boost/filesystem/fstream.hpp>
#include <boost/serialization/set.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>
#include <boost/serialization/vector.hpp>

#include <iostream>

using namespace ouisync;
using namespace std;
using object::Tree;
using object::Blob;

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

    hash.update(uint32_t(_nodes.size()));
    for (auto& [id, n] : _nodes) {
        hash.update(static_cast<std::underlying_type_t<NodeType>>(n.type));
        hash.update(id);
    }

    return hash.close();
}

Snapshot::Snapshot(ObjectStore& objects, fs::path snapshotdir, Commit commit) :
    _name_tag(_generate_random_name_tag()),
    _path(_path_from_tag(_name_tag, snapshotdir)),
    _objects(&objects),
    _snapshotdir(std::move(snapshotdir)),
    _commit(move(commit))
{
    _nodes.insert({_commit.root_id, {NodeType::Missing, {}, {}}});
}

Snapshot::Snapshot(Snapshot&& other) :
    _name_tag(other._name_tag),
    _path(std::move(other._path)),
    _objects(other._objects),
    _snapshotdir(std::move(other._snapshotdir)),
    _commit(move(other._commit)),
    _nodes(move(other._nodes))
{
}

Snapshot& Snapshot::operator=(Snapshot&& other)
{
    forget();

    _name_tag = other._name_tag;
    _path     = std::move(other._path);
    _objects  = other._objects;
    _commit   = std::move(other._commit);
    _nodes  = move(other._nodes);

    return *this;
}

/* static */
Snapshot Snapshot::create(Commit commit, ObjectStore& objects, Options::Snapshot options)
{
    Snapshot s(objects, move(options.snapshotdir), std::move(commit));

    s.store();
    return s;
}

bool Snapshot::check_complete_and_notify_parents(const ObjectId& node_id)
{
    auto& node = _nodes.at(node_id);

    // Can't be Complete as only Incomplete childs call this function.
    ouisync_assert(node.type == NodeType::Incomplete);

    if (!check_all_complete(node.children))
        return false;

    node.type = NodeType::Complete;

    // Held by this Snapshot
    increment_recursive_count(node_id);
    decrement_direct_count(node_id);

    // Keep copies as code below may erase `node` from `_nodes` while iterating
    // over `children` or `parents`.
    auto children = node.children;
    auto parents  = node.parents;

    for (auto& child_id : children) {
        increment_recursive_count(child_id);

        auto& child = _nodes.at(child_id);

        if (check_all_complete(child.parents)) {
            // Remove the refcound held by this snapshot and stop tracking this
            // node.
            decrement_recursive_count(child_id);
            _nodes.erase(child_id);
        }
    }

    for (auto& parent_id : parents) {
        check_complete_and_notify_parents(parent_id);
    }

    return true;
}

std::set<ObjectId> Snapshot::children_of(const ObjectId& id) const
{
    auto obj = _objects->load<Tree, Blob::Nothing>(id);
    if (auto tree = boost::get<Tree>(&obj)) {
        return tree->children();
    }
    return {};
}

void Snapshot::increment_recursive_count(const ObjectId& id) const
{
    _objects->rc(id).increment_recursive_count();
}

void Snapshot::increment_direct_count(const ObjectId& id) const
{
    _objects->rc(id).increment_direct_count();
}

void Snapshot::decrement_recursive_count(const ObjectId& id) const
{
    _objects->rc(id).decrement_recursive_count();
}

void Snapshot::decrement_direct_count(const ObjectId& id) const
{
    _objects->rc(id).decrement_direct_count();
}

bool Snapshot::check_all_complete(const set<ObjectId>& nodes)
{
    for (auto& id : nodes) {
        if (_nodes.at(id).type != NodeType::Complete)
            return false;
    }
    return true;
}

void Snapshot::insert_object(const ObjectId& node_id, set<ObjectId> children)
{
    auto i = _nodes.find(node_id);

    // This can happen if:
    // A. this function is called with node_id of a node whose parent hasn't been
    //    inserted yet, or
    // B. This function is called with node_id of a node which has been previously
    //    inserted, it was decided it is complete, all its parents were complete and
    //    thus it the node was removed from _nodes.
    if (i == _nodes.end()) return;

    if (i->second.type != NodeType::Missing) return;

    auto& node = i->second;

    node.type = NodeType::Incomplete;

    increment_direct_count(node_id);

    // Don't move because later code may remove `node` from `_nodes` while
    // iteratirng over children.
    node.children = children;

    for (auto& child_id : node.children) {
        auto [child_i, inserted] = _nodes.insert({child_id, {NodeType::Missing, {}, {}}});
        child_i->second.parents.insert(node_id);
    }

    size_t existing_children = 0;

    for (auto& child_id : children) {
        if (!_objects->exists(child_id)) continue;
        existing_children++;
        insert_object(child_id, children_of(child_id));
    }

    if (existing_children == 0) {
        check_complete_and_notify_parents(node_id);
    }
}

void Snapshot::store()
{
    archive::store(_path, _nodes);
}

void Snapshot::forget() noexcept
{
    if (_nodes.empty()) return;

    auto nodes = move(_nodes);

    try {
        for (auto& [id, node] : nodes) {
            switch (node.type) {
                case NodeType::Complete: {
                    _objects->rc(id).decrement_recursive_count();
                }
                break;

                case NodeType::Incomplete: {
                    _objects->rc(id).decrement_direct_count();
                }
                break;

                case NodeType::Missing: {
                }
                break;
            }
        }
    }
    catch (const std::exception& e) {
        exit(1);
    }
}

Snapshot Snapshot::clone() const
{
    Snapshot c(*_objects, _snapshotdir, _commit);

    for (auto& [id, node] : _nodes) {
        c._nodes.insert({id, node});

        switch (node.type) {
            case NodeType::Complete: {
                increment_recursive_count(id);
            }
            break;

            case NodeType::Incomplete: {
                increment_direct_count(id);
            }
            break;

            case NodeType::Missing: {
            }
            break;
        }
    }

    return c;
}

void Snapshot::sanity_check() const
{
    //for (auto& [id, _] : _objects.incomplete) {
    //    ouisync_assert(object::io::exists(_objdir, id));
    //}

    //for (auto& id : _objects.complete) {
    //    ouisync_assert(object::io::is_complete(_objdir, id));
    //}
}

Snapshot::~Snapshot()
{
    forget();
}

std::ostream& ouisync::operator<<(std::ostream& os, const Snapshot& s)
{
    os << "Snapshot: " << s._commit << "\n";
    os << BranchView(const_cast<ObjectStore&>(*s._objects), s._commit.root_id);
    if (s._nodes.empty()) {
        os << "EMPTY!!!\n";
    }
    for (auto& [id, n] : s._nodes) {
        os << id << ": " << n << "\n";
    }
    return os;
}

std::ostream& ouisync::operator<<(std::ostream& os, Snapshot::NodeType t)
{
    switch (t) {
        case Snapshot::NodeType::Missing:    return os << "Missing";
        case Snapshot::NodeType::Incomplete: return os << "Incomplete";
        case Snapshot::NodeType::Complete:   return os << "Complete";
    }
    return os;
}

std::ostream& ouisync::operator<<(std::ostream& os, const Snapshot::Node& n)
{
    os << "Node{" << n.type << ", ";
    os << "parents: " << n.parents << ", ";
    os << "children: " << n.children << "}";
    return os;
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
