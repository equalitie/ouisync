#pragma once

#include <string>
#include <map>
#include <vector>

namespace ouisync {

struct MonitorId {
    std::string name;
    size_t disambiguator;
};

struct StateMonitorNode {
    std::map<std::string, std::string> values;
    std::vector<MonitorId> children;
};

} // namespace ouisync

#include <ouisync/data.g.hpp>

