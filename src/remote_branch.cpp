#include "remote_branch.h"
#include "branch_io.h"
#include "object/io.h"
#include "object/refcount.h"

#include <boost/archive/binary_iarchive.hpp>
#include <boost/archive/binary_oarchive.hpp>
#include <boost/serialization/vector.hpp>
#include <boost/serialization/set.hpp>

using namespace ouisync;

using std::move;
using std::make_pair;
using object::Id;
using object::Tree;
using std::set;

template<class Obj>
static
Id _flat_store(const fs::path& objdir, const Obj& obj) {
    auto new_id = object::io::store(objdir, obj);
    object::refcount::increment(objdir, new_id);
    return new_id;
}

RemoteBranch::RemoteBranch(Commit commit, fs::path filepath, fs::path objdir) :
    _filepath(std::move(filepath)),
    _objdir(std::move(objdir)),
    _root(commit.root_object_id),
    _version_vector(commit.version_vector)
{
}

RemoteBranch::RemoteBranch(fs::path filepath, fs::path objdir, IArchive& ar) :
    _filepath(std::move(filepath)),
    _objdir(std::move(objdir))
{
    load_rest(ar);
}

net::awaitable<void> RemoteBranch::mark_complete(const Id& id)
{
    auto i = _objects.find(id);
    assert(i != _objects.end());

    if (i == _objects.end()) {
        throw std::runtime_error("RemoteBranch::mark_complete: id not found");
    }

    auto& entry = i->second;

    entry.state = ObjectState::Complete;

    for (auto& child : entry.children) {
        _objects.erase(child);
    }

    co_return store_self();
}

net::awaitable<void> RemoteBranch::insert_blob(const Blob& blob)
{
    auto id = _flat_store(_objdir, blob);
    co_await insert_object(id, {});
}

net::awaitable<Id> RemoteBranch::insert_tree(const Tree& tree)
{
    auto id = _flat_store(_objdir, tree);

    set<Id> children;
    for (auto [name, hash] : tree) {
        (void) name;
        children.insert(hash);
    }

    co_await insert_object(id, std::move(children));
    co_return id;
}

net::awaitable<void> RemoteBranch::insert_object(const Id& objid, set<Id> children)
{
    bool inserted =
        _objects.insert(make_pair(objid,
                ObjectEntry{ObjectState::Incomplete, std::move(children)})).second;

    assert(inserted);
    co_return store_self();
}

Tree RemoteBranch::readdir(PathRange path) const
{
    return BranchIo::readdir(_objdir, _root, path);
}

FileSystemAttrib RemoteBranch::get_attr(PathRange path) const
{
    return BranchIo::get_attr(_objdir, _root, path);
}

size_t RemoteBranch::read(PathRange path, const char* buf, size_t size, size_t offset) const
{
    return BranchIo::read(_objdir, _root, path, buf, size, offset);
}

//--------------------------------------------------------------------

void RemoteBranch::store_self() const {
    fs::fstream file(_filepath, file.binary | file.trunc | file.out);

    if (!file.is_open())
        throw std::runtime_error("Failed to open branch file");

    OArchive oa(file);

    store_tag(oa);
    store_rest(oa);
}

void RemoteBranch::store_tag(OArchive& ar) const
{
    ar << BranchIo::BranchType::Remote;
}

void RemoteBranch::store_rest(OArchive& ar) const
{
    ar << _root;
    ar << _version_vector;
    ar << _objects;
}

void RemoteBranch::load_rest(IArchive& ar)
{
    ar >> _root;
    ar >> _version_vector;
    ar >> _objects;
}

//--------------------------------------------------------------------
