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
    std::cerr << "Inserting blob " << blob.calculate_id() << " " << _commit << "\n";
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
    std::cerr << "Inserting tree " << tree.calculate_id() << " " << _commit << "\n";
    return insert_object(tree, _children(tree));
}

template<class Obj>
net::awaitable<ObjectId> RemoteBranch::insert_object(const Obj& obj, std::set<ObjectId> children)
{
    [[maybe_unused]] auto id = _flat_store(_objdir, obj);

    if (!children.empty()) {
        for (auto& child : children) {
            auto i = _missing_objects.insert({child, {}}).first;
            i->second.insert(id);
        }

        _incomplete_objects.insert({id, move(children)});
    }

    auto i = _missing_objects.find(id);

    if (i == _missing_objects.end()) {
        throw std::runtime_error("The Object is not missing");
    }

    for (auto& parent : i->second) {
        auto incomplete_i = _incomplete_objects.find(parent);

        if (incomplete_i == _incomplete_objects.end()) {
            throw std::runtime_error("No such incomplete object");
        }

        incomplete_i->second.erase(i->first);

        if (incomplete_i->second.empty()) {
            _incomplete_objects.erase(incomplete_i);
        }
    }

    store_self();
    co_return id;
}

net::awaitable<void> RemoteBranch::introduce_commit(const Commit& commit)
{
    _commit = commit;

    // XXX: TODO
    _missing_objects.clear();
    _incomplete_objects.clear();

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
