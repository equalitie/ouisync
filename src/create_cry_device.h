#pragma once

#include "shortcuts.h"
#include <boost/filesystem/path.hpp>

// This is likely just a temporary helper function to make creation of
// the cry device simpler.

namespace fspp { class Device; }

namespace ouisync {

std::unique_ptr<fspp::Device> create_cry_device(const fs::path& root, bool test=false);

} // namespace
