#pragma once

#include <sstream>
#include <vector>
#include <ouisync/data.hpp>
#include <ouisync/message.hpp>

namespace ouisync {

std::stringstream serialize(const Request&);

ResponseResult deserialize(const std::vector<char>&);

} // namespace ouisync
