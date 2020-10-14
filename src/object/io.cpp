#include "io.h"
#include "block.h"
#include "tree.h"
#include "../defer.h"

#include "../hex.h"
#include "../array_io.h"

#include <boost/range/iterator_range.hpp>
#include <boost/system/error_code.hpp>
#include <boost/filesystem/operations.hpp>

namespace ouisync::object::io {

namespace {
    using PathRange = boost::iterator_range<fs::path::iterator>;

    PathRange path_range(const fs::path& path) {
        return boost::make_iterator_range(path);
    }

    // Debug
    std::ostream& operator<<(std::ostream& os, PathRange r) {
        fs::path path;
        for (auto& p : r) path /= p;
        return os << path;
    }

    fs::path refcount_path(const Id& id) noexcept {
        return path::from_id(id).concat(".rc");
    }

    std::string debug(const fs::path& p) noexcept {
        fs::ifstream t(p);
        return std::string((std::istreambuf_iterator<char>(t)),
                            std::istreambuf_iterator<char>());
    }

    uint32_t increment_refcount(const fs::path& objdir, const Id& id)
    {
        auto path = objdir / refcount_path(id);
        fs::fstream f(path, f.binary | f.in | f.out);
        if (!f.is_open()) {
            // Does not exist, create a new one
            f.open(path, f.binary | f.out | f.trunc);
            if (!f.is_open()) {
                throw std::runtime_error("Failed to increment refcount");
            }
            f << 1 << '\n';
            return 1;
        }
        uint32_t rc;
        f >> rc;
        ++rc;
        f.seekp(0);
        f << rc << '\n';
        return rc;
    }

    uint32_t decrement_refcount(const fs::path& objdir, const Id& id)
    {
        auto path = objdir / refcount_path(id);
        fs::fstream f(path, f.binary | f.in | f.out);
        if (!f.is_open()) {
            if (!fs::exists(path)) {
                // No one held this object
                return 0;
            }
            throw std::runtime_error("Failed to decrement refcount");
        }
        uint32_t rc;
        f >> rc;
        if (rc == 0) throw std::runtime_error("Decrementing zero refcount");
        --rc;
        if (rc == 0) {
            f.close();
            fs::remove(path);
            return 0;
        }
        f.seekp(0);
        f << rc;
        return rc;
    }
}

RefCount refcount(const fs::path& objdir, const Id& id) {
    auto path = objdir / refcount_path(id);
    fs::fstream f(path, f.binary | f.in);
    if (!f.is_open()) {
        if (!fs::exists(path)) {
            // No one is holding this object
            return 0;
        }
        throw std::runtime_error("Failed to decrement refcount");
    }
    uint32_t rc;
    f >> rc;
    return rc;
}

static
Opt<Id> _store_recur(const fs::path& objdir, Opt<Id> old_object_id, PathRange path, const Block &block, bool replace)
{
    Opt<Id> obj_id;

    auto on_exit = defer([&] {
            if (old_object_id && obj_id) flat::remove(objdir, *old_object_id);
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

    increment_refcount(objdir, *obj_id);
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

bool flat::remove(const fs::path& objdir, const Id& id) {
    auto rc = decrement_refcount(objdir, id);
    if (rc > 0) return true;
    sys::error_code ec;
    fs::remove(objdir/path::from_id(id), ec);
    return !ec;
}

bool remove(const fs::path& objdir, const Id& id) {
    auto obj = load<Tree, JustTag<Block>>(objdir, id);

    apply(obj,
            [&](const Tree& tree) {
                for (auto& [name, id] : tree) {
                    (void)name; // https://stackoverflow.com/a/40714311/273348
                    remove(objdir, id);
                }
            },
            [&](const JustTag<Block>&) {
            });

    return flat::remove(objdir, id);
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

    flat::remove(objdir, tree_id);
    return tree->store(objdir);
}

Opt<Id> remove(const fs::path& objdir, Id root, const fs::path& path) {
    return _remove_recur(objdir, root, path_range(path));
}

} // namespace
