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
}

static
Id _store_recur(const fs::path& objdir, Opt<Id> old_object_id, PathRange path, const Block &block)
{
    auto on_exit = defer([&] {
            if (old_object_id) remove(objdir, *old_object_id);
        });

    if (path.empty()) {
        return block.store(objdir);
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

    child_i->second = _store_recur(objdir, old_child_id, path, block);

    return tree.store(objdir);
}

Id store(const fs::path& objdir, Id root_tree, const fs::path& objpath, const Block& block)
{
    return _store_recur(
            objdir,
            root_tree,
            boost::make_iterator_range(objpath),
            block);
}

bool remove(const fs::path& objdir, const Id& id) {
    auto p = path::from_id(id);
    sys::error_code ec;
    fs::remove(objdir/p, ec);
    return bool(ec);
}

} // namespace
