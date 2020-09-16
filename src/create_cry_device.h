#pragma once

#include <namespaces.h>
#include <boost/filesystem.hpp>

// This is likely just a temporary helper function to make creation of
// the cry device simpler.

namespace cryfs { class CryDevice; }

namespace ouisync {

std::unique_ptr<cryfs::CryDevice> create_cry_device(const fs::path& root);

} // namespace
