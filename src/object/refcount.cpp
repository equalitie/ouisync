#include "refcount.h"
#include "path.h"
#include <boost/filesystem/fstream.hpp>
#include <boost/filesystem/operations.hpp>

namespace ouisync::object::refcount {

static
fs::path refcount_path(const Id& id) noexcept {
    return path::from_id(id).concat(".rc");
}

Number increment(const fs::path& objdir, const Id& id)
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
    Number rc;
    f >> rc;
    ++rc;
    f.seekp(0);
    f << rc << '\n';
    return rc;
}

Number decrement(const fs::path& objdir, const Id& id)
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
    Number rc;
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

Number read(const fs::path& objdir, const Id& id) {
    auto path = objdir / refcount_path(id);
    fs::fstream f(path, f.binary | f.in);
    if (!f.is_open()) {
        if (!fs::exists(path)) {
            // No one is holding this object
            return 0;
        }
        throw std::runtime_error("Failed to decrement refcount");
    }
    Number rc;
    f >> rc;
    return rc;
}

} // namespace
