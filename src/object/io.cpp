#include "io.h"
#include "block.h"
#include "tree.h"
#include "../defer.h"

#include <boost/range/iterator_range.hpp>
#include <boost/system/error_code.hpp>
#include <boost/filesystem/operations.hpp>

namespace ouisync::object::io {

namespace {
    using PathRange = boost::iterator_range<fs::path::iterator>;

    PathRange path_range(const fs::path& path) {
        return boost::make_iterator_range(path);
    }
}

static
Opt<Id> _store_recur(const fs::path& objdir, Opt<Id> old_object_id, PathRange path, const Block &block, bool replace)
{
    Opt<Id> obj_id;

    auto on_exit = defer([&] {
            if (old_object_id && obj_id) remove(objdir, *old_object_id);
        });

    if (path.empty()) {
        if (replace) obj_id = store(objdir, block);
        else         obj_id = maybe_store(objdir, block);
        return obj_id;
    }

    auto child_name = path.front().string();
    path.advance_begin(1);

    Tree tree;

    if (old_object_id) {
        tree = load<Tree>(objdir, *old_object_id);
    }

    auto [child_i, inserted] = tree.insert(std::make_pair(child_name, Id{}));

    Opt<Id> old_child_id;
    if (!inserted) old_child_id = child_i->second;

    obj_id = _store_recur(objdir, old_child_id, path, block, replace);

    if (!obj_id) return boost::none;

    child_i->second = *obj_id;
    return tree.store(objdir);
}

Id store(const fs::path& objdir, Id root_tree, const fs::path& objpath, const Block& block)
{
    auto id = _store_recur(objdir, root_tree, path_range(objpath), block, true);
    assert(id);
    if (!id) throw std::runtime_error("Object not replaced as it should have been");
    return *id;
}

Opt<Id> maybe_store(const fs::path& objdir, Id root_tree, const fs::path& objpath, const Block& block)
{
    return _store_recur(objdir, root_tree, path_range(objpath), block, false);
}

inline
Block _load_recur(const fs::path& objdir, const Id& root_id, PathRange path) {
    if (path.empty())
        throw std::runtime_error("Can't load object without name");

    auto tree = load<Tree>(objdir, root_id);
    auto name = path.front().string();

    auto i = tree.find(name);

    if (i == tree.end()) {
        throw std::runtime_error("Block not found");
    }

    if (path.advance_begin(1); path.empty()) {
        return load<Block>(objdir, i->second);
    } else {
        return _load_recur(objdir, i->second, path);
    }
}

Block load(const fs::path& objdir, const Id& root_id, const fs::path& objpath) {
    return _load_recur(objdir, root_id, path_range(objpath));
}

bool remove(const fs::path& objdir, const Id& id) {
    auto p = path::from_id(id);
    sys::error_code ec;
    // XXX: If this is a tree, remove it's children as well.
    fs::remove(objdir/p, ec);
    return bool(ec);
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

    auto tree = maybe_load<Tree>(objdir, tree_id);
    if (!tree) return boost::none;

    auto i = tree->find(child_name);
    if (i == tree->end()) return boost::none;

    if (path.empty()) {
        if (!remove(objdir, i->second)) return boost::none;
        tree->erase(i);
    } else {
        auto opt_new_id = _remove_recur(objdir, i->second, path);
        if (!opt_new_id) return boost::none;
        i->second = *opt_new_id;
    }

    remove(objdir, tree_id);
    return tree->store(objdir);
}

Opt<Id> remove(const fs::path& objdir, Id root, const fs::path& path) {
    return _remove_recur(objdir, root, path_range(path));
}

} // namespace
