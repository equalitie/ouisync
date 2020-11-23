#include "io.h"

#include <boost/system/error_code.hpp>
#include <boost/filesystem/operations.hpp>

namespace ouisync::object::io {

bool remove(const fs::path& objdir, const Id& id) {
    sys::error_code ec;
    fs::remove(objdir/path::from_id(id), ec);
    return !ec;
}

bool exists(const fs::path& objdir, const Id& id) {
    return fs::exists(objdir/path::from_id(id));
}

} // namespace
