#pragma once

#include <string>
#include <map>
#include <vector>

namespace ouisync {

struct MonitorId {
    std::string name;
    size_t disambiguator;
};

inline bool operator == (const MonitorId& lhs, const MonitorId& rhs) {
    return lhs.name == rhs.name && lhs.disambiguator == rhs.disambiguator;
}

inline bool operator != (const MonitorId& lhs, const MonitorId& rhs) {
    return !(lhs == rhs);
}

struct StateMonitorNode {
    std::map<std::string, std::string> values;
    std::vector<MonitorId> children;
};

inline bool operator == (const StateMonitorNode& lhs, const StateMonitorNode& rhs) {
    return lhs.values == rhs.values && lhs.children == rhs.children;
}

inline bool operator != (const StateMonitorNode& lhs, const StateMonitorNode& rhs) {
    return !(lhs == rhs);
}

} // namespace ouisync

#include <ouisync/data.g.hpp>
