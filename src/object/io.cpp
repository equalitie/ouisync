#include "io.h"

#include <boost/system/error_code.hpp>
#include <boost/filesystem/operations.hpp>

namespace ouisync::object {

bool remove(const fs::path& objdir, const Id& id) {
    auto p = path::from_id(id);
    sys::error_code ec;
    fs::remove(objdir/p, ec);
    return bool(ec);
}

} // namespace
