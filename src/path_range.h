#pragma once

#include "path.h"
#include <boost/range/iterator_range.hpp>

namespace ouisync {

//using Path = std::vector<std::string>;
using PathRange = boost::iterator_range<Path::const_iterator>;

inline
PathRange path_range(const Path& path) {
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
