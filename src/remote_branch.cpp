#include "remote_branch.h"
#include "branch_io.h"
#include "branch_type.h"
#include "object/io.h"
#include "refcount.h"

#include <boost/archive/binary_iarchive.hpp>
#include <boost/archive/binary_oarchive.hpp>
#include <boost/serialization/vector.hpp>
#include <boost/serialization/set.hpp>

#include <iostream>

using namespace ouisync;

using std::move;
using std::make_pair;
using object::Tree;
using std::set;

template<class Obj>
static
ObjectId _flat_store(const fs::path& objdir, const Obj& obj) {
    auto new_id = object::io::store(objdir, obj);
    refcount::increment(objdir, new_id);
    return new_id;
}

RemoteBranch::RemoteBranch(Commit commit, fs::path filepath, fs::path objdir) :
    _filepath(std::move(filepath)),
    _objdir(std::move(objdir)),
    _commit(std::move(commit))
{
    _missing_objects.insert({_commit.root_id, {}});
}

RemoteBranch::RemoteBranch(fs::path filepath, fs::path objdir, IArchive& ar) :
    _filepath(std::move(filepath)),
    _objdir(std::move(objdir))
{
    load_body(ar);
}

net::awaitable<ObjectId> RemoteBranch::insert_blob(const Blob& blob)
{
    return insert_object(blob, {});
}

static std::set<ObjectId> _children(const Tree& tree)
{
    std::set<ObjectId> ret;
    for (auto& ch : tree) ret.insert(ch.second);
    return ret;
}

net::awaitable<ObjectId> RemoteBranch::insert_tree(const Tree& tree)
{
    return insert_object(tree, _children(tree));
}

void RemoteBranch::filter_missing(std::set<ObjectId>& objs) const
{
    for (auto i = objs.begin(); i != objs.end();) {
        auto j = std::next(i);
        if (object::io::exists(_objdir, *i)) objs.erase(i);
        i = j;
    }
}

template<class Obj>
net::awaitable<ObjectId> RemoteBranch::insert_object(const Obj& obj, std::set<ObjectId> children)
{
    // Missing objects:    Object -> Parents
    // Incomplete objects: Object -> Children

    auto id = obj.calculate_id();

    auto parents = std::move(_missing_objects.at(id));
    _missing_objects.erase(id);

    if (children.empty()) {
        _complete_objects.insert(id);
    } else {
        filter_missing(children);

        for (auto& child : children) {
            _missing_objects[child].insert(id);
        }

        if (children.empty()) {
            _complete_objects.insert(id);
        } else {
            _incomplete_objects.insert({id, move(children)});
        }
    }

    _flat_store(_objdir, obj);

    // Check that any of the parents of `obj` became "complete".
    for (auto& parent : parents) {
        auto& missing_children = _incomplete_objects.at(parent);

        missing_children.erase(id);

        if (missing_children.empty()) {
            // Reference counting stays the same
            _incomplete_objects.erase(id);
            _complete_objects.insert(id);
        }
    }

    store_self();
    co_return id;
}

net::awaitable<void> RemoteBranch::introduce_commit(const Commit& commit)
{
    _commit = commit;

    // Missing objects don't increase refcount
    _missing_objects.clear();

    auto incomplete_objects = std::move(_incomplete_objects);
    auto complete_objects   = std::move(_complete_objects);

    for (auto& [id, _] : incomplete_objects) refcount::deep_remove(_objdir, id);
    for (auto& id      : complete_objects)   refcount::deep_remove(_objdir, id);

    _missing_objects.insert({_commit.root_id, {}});

    store_self();
    co_return;
}

//--------------------------------------------------------------------

void RemoteBranch::store_self() const {
    fs::fstream file(_filepath, file.binary | file.trunc | file.out);

    if (!file.is_open())
        throw std::runtime_error(str(boost::format("Failed to open branch file %1%") % _filepath));

    OArchive oa(file);

    store_tag(oa);
    store_body(oa);
}

void RemoteBranch::store_tag(OArchive& ar) const
{
    ar << BranchType::Remote;
}

void RemoteBranch::store_body(OArchive& ar) const
{
    ar << _commit;
    ar << _missing_objects;
    ar << _incomplete_objects;
}

void RemoteBranch::load_body(IArchive& ar)
{
    ar >> _commit;
    ar >> _missing_objects;
    ar >> _incomplete_objects;
}

//--------------------------------------------------------------------
