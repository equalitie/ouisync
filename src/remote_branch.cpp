#include "remote_branch.h"
#include "branch_io.h"
#include "branch_type.h"
#include "object/io.h"
#include "object/refcount.h"

#include <boost/archive/binary_iarchive.hpp>
#include <boost/archive/binary_oarchive.hpp>
#include <boost/serialization/vector.hpp>
#include <boost/serialization/set.hpp>

#include <iostream>

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
    _commit(std::move(commit))
{
}

RemoteBranch::RemoteBranch(fs::path filepath, fs::path objdir, IArchive& ar) :
    _filepath(std::move(filepath)),
    _objdir(std::move(objdir))
{
    load_rest(ar);
}

net::awaitable<void> RemoteBranch::insert_blob(const Blob& blob)
{
    [[maybe_unused]] auto id = _flat_store(_objdir, blob);
    // XXX: TODO
    store_self();
    co_return;
}

net::awaitable<Id> RemoteBranch::insert_tree(const Tree& tree)
{
    [[maybe_unused]] auto id = _flat_store(_objdir, tree);
    // XXX: TODO
    store_self();
    co_return id;
}

net::awaitable<void> RemoteBranch::introduce_commit(const Commit& commit)
{
    _commit = commit;
    // XXX: TODO
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
    store_rest(oa);
}

void RemoteBranch::store_tag(OArchive& ar) const
{
    ar << BranchType::Remote;
}

void RemoteBranch::store_rest(OArchive& ar) const
{
    ar << _commit;
}

void RemoteBranch::load_rest(IArchive& ar)
{
    ar >> _commit;
}

//--------------------------------------------------------------------
