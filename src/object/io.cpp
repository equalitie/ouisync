#include "io.h"
#include "blob.h"
#include "tree.h"
#include "../variant.h"
#include "../refcount.h"

#include <boost/system/error_code.hpp>
#include <boost/filesystem/operations.hpp>

namespace ouisync::object::io {

bool remove(const fs::path& objdir, const ObjectId& id) {
    sys::error_code ec;
    fs::remove(objdir/path::from_id(id), ec);
    return !ec;
}

bool exists(const fs::path& objdir, const ObjectId& id) {
    return fs::exists(objdir/path::from_id(id));
}

bool is_complete(const fs::path& objdir, const ObjectId& id) {
    if (!exists(objdir, id)) return false;

    auto v = load<Blob, Tree>(objdir, id);

    return apply(v,
        [&] (const Blob&) { return true; },
        [&] (const Tree& tree) {
            for (auto& [_, id] : tree) {
                if (!is_complete(objdir, id)) return false;
            }
            return true;
        });
}

} // namespace
