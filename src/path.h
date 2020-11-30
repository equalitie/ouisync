#pragma once

#include "shortcuts.h"
#include <boost/filesystem/path.hpp>

namespace ouisync {

/*
 * Using this class instead of boost::filesystem::path (fs::path) because
 * fs::path has a terrible iterator implementation: Each such iterator holds a
 * separate instance of fs::path in it. This means that the fs::path inside it
 * get's destroyed right after the iterator itself is destroyed (which also
 * happens on e.g. incrementing the iterator).
 *
 * Last I checked this was the case with Boost 1.74. I believe std
 * implementations also have this problem maybe apart the most recent ones.
 */
class Path : public std::vector<std::string> {
private:
    using Parent = std::vector<std::string>;

public:
    using Parent::Parent;
    using Parent::iterator;
    using Parent::const_iterator;

    Path(const fs::path& path) {
        for (auto& part : path) { push_back(part.native()); }
    }
};

} // namespace
