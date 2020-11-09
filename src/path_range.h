#pragma once

#include <boost/filesystem/path.hpp>
#include <boost/range/iterator_range.hpp>

namespace ouisync {

using PathRange = boost::iterator_range<fs::path::iterator>;

inline
PathRange path_range(const fs::path& path) {
    return boost::make_iterator_range(path);
}

namespace {
    // Debug
    inline
    std::ostream& __attribute__((unused)) operator<<(std::ostream& os, PathRange r) {
        fs::path path;
        for (auto& p : r) path /= p;
        return os << path;
    }
}

} // namespace
