#include "branch.h"
#include "variant.h"
#include "object/tree.h"
#include "object/tagged.h"
#include "object/block.h"
#include "object/io.h"
#include "object/refcount.h"


#include <boost/filesystem.hpp>
#include <boost/archive/text_oarchive.hpp>
#include <boost/archive/text_iarchive.hpp>
#include <boost/serialization/vector.hpp>
#include <boost/range/iterator_range.hpp>

using namespace ouisync;
using std::move;
using object::Id;
using object::Block;
using object::Tree;
using object::JustTag;
using Data = Branch::Data;

namespace {
    using PathRange = boost::iterator_range<fs::path::iterator>;

    PathRange path_range(const fs::path& path) {
        return boost::make_iterator_range(path);
    }

    // Debug
    //std::ostream& operator<<(std::ostream& os, PathRange r) {
    //    fs::path path;
    //    for (auto& p : r) path /= p;
    //    return os << path;
    //}

    // Debug
    //std::string debug(const fs::path& p) noexcept {
    //    fs::ifstream t(p);
    //    return std::string((std::istreambuf_iterator<char>(t)),
    //                        std::istreambuf_iterator<char>());
    //}
}

/* static */
Branch Branch::load_or_create(const fs::path& rootdir, const fs::path& objdir, UserId user_id) {
    object::Id root_id;
    VersionVector clock;

    fs::path path = rootdir / user_id.to_string();

    fs::fstream file(path, file.binary | file.in);

    if (!file.is_open()) {
        object::Tree root_obj;
        root_id = root_obj.store(objdir);
        object::refcount::increment(objdir, root_id);
        Branch branch(path, objdir, user_id, root_id, std::move(clock));
        branch.store_self();
        return branch;
    }

    boost::archive::text_iarchive oa(file);
    object::tagged::Load<object::Id> load{root_id};
    oa >> load;
    oa >> clock;

    return Branch{path, objdir, user_id, root_id, move(clock)};
}

//--------------------------------------------------------------------

static
bool _flat_remove(const fs::path& objdir, const Id& id) {
    auto rc = object::refcount::decrement(objdir, id);
    if (rc > 0) return true;
    return object::io::remove(objdir, id);
}

//--------------------------------------------------------------------

static
Opt<Id> _store_recur(const fs::path& objdir, Opt<Id> old_object_id, PathRange path, const Branch::Data &block, bool replace)
{
    Opt<Id> obj_id;

    if (path.empty()) {
        if (replace) obj_id = object::io::store(objdir, block);
        else         obj_id = object::io::maybe_store(objdir, block);
        if (old_object_id && obj_id) _flat_remove(objdir, *old_object_id);
        return obj_id;
    }

    auto child_name = path.front().string();
    path.advance_begin(1);

    Tree tree;

    if (old_object_id) {
        tree = object::io::load<Tree>(objdir, *old_object_id);
    }

    auto [child_i, inserted] = tree.insert(std::make_pair(child_name, Id{}));

    Opt<Id> old_child_id;
    if (!inserted) old_child_id = child_i->second;

    obj_id = _store_recur(objdir, old_child_id, path, block, replace);

    if (!obj_id) return boost::none;

    object::refcount::increment(objdir, *obj_id);
    child_i->second = *obj_id;
    if (old_object_id) _flat_remove(objdir, *old_object_id);
    return tree.store(objdir);
}

void Branch::store(const fs::path& path, const Data& data)
{
    auto oid = _store_recur(_objdir, root_object_id(), path_range(path), data, true);
    assert(oid);
    if (!oid) throw std::runtime_error("Object not replaced as it should have been");
    root_object_id(*oid);
}

bool Branch::maybe_store(const fs::path& path, const Data& data)
{
    auto oid = _store_recur(_objdir, root_object_id(), path_range(path), data, false);
    if (!oid) return false;
    root_object_id(*oid);
    return true;
}

//--------------------------------------------------------------------

inline
Opt<Branch::Data> _maybe_load_recur(const fs::path& objdir, const Id& root_id, PathRange path) {
    if (path.empty())
        throw std::runtime_error("Can't load object without name");

    auto tree = object::io::maybe_load<Tree>(objdir, root_id);
    if (!tree) return boost::none;

    auto name = path.front().string();

    auto i = tree->find(name);

    if (i == tree->end()) {
        throw std::runtime_error("Block not found");
    }

    if (path.advance_begin(1); path.empty()) {
        return object::io::maybe_load<Branch::Data>(objdir, i->second);
    } else {
        return _maybe_load_recur(objdir, i->second, path);
    }
}

Opt<Branch::Data> Branch::maybe_load(const fs::path& path) const
{
    auto ob = _maybe_load_recur(_objdir, root_object_id(), path_range(path));
    if (!ob) return boost::none;
    return move(*ob);
}

//--------------------------------------------------------------------

static
bool _remove_with_children(const fs::path& objdir, const Id& id) {
    auto obj = object::io::load<Tree, JustTag<Data>>(objdir, id);

    apply(obj,
            [&](const Tree& tree) {
                for (auto& [name, id] : tree) {
                    (void)name; // https://stackoverflow.com/a/40714311/273348
                    _remove_with_children(objdir, id);
                }
            },
            [&](const JustTag<Data>&) {
            });

    return _flat_remove(objdir, id);
}

static
Opt<Id> _remove_recur(const fs::path& objdir, Id tree_id, PathRange path)
{
    if (path.empty()) {
        // Assuming the root obj can't be deleted with this function.
        return boost::none;
    }

    auto child_name = path.front().string();
    path.advance_begin(1);

    auto tree = object::io::maybe_load<Tree>(objdir, tree_id);
    if (!tree) return boost::none;

    auto i = tree->find(child_name);
    if (i == tree->end()) return boost::none;

    if (path.empty()) {
        if (!_remove_with_children(objdir, i->second)) return boost::none;
        tree->erase(i);
    } else {
        auto opt_new_id = _remove_recur(objdir, i->second, path);
        if (!opt_new_id) return boost::none;
        i->second = *opt_new_id;
    }

    _flat_remove(objdir, tree_id);
    return tree->store(objdir);
}

bool Branch::remove(const fs::path& path)
{
    auto oid = _remove_recur(_objdir, root_object_id(), path_range(path));
    if (!oid) return false;
    root_object_id(*oid);
    return true;
}

//--------------------------------------------------------------------

void Branch::store_self() const {
    fs::fstream file(_file_path, file.binary | file.trunc | file.out);
    if (!file.is_open())
        throw std::runtime_error("Failed to open branch file");
    boost::archive::text_oarchive oa(file);
    object::tagged::Save<object::Id> save{_root_id};
    oa << save;
    oa << _clock;
}

//--------------------------------------------------------------------

void Branch::root_object_id(const object::Id& id) {
    if (_root_id == id) return;
    _root_id = id;
    _clock.increment(_user_id);
    store_self();
}

//--------------------------------------------------------------------

Branch::Branch(const fs::path& file_path, const fs::path& objdir,
        const UserId& user_id, const object::Id& root_id, VersionVector clock) :
    _file_path(file_path),
    _objdir(objdir),
    _user_id(user_id),
    _root_id(root_id),
    _clock(std::move(clock))
{}

//--------------------------------------------------------------------

