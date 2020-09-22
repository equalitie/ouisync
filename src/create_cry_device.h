#pragma once

#include <namespaces.h>
#include <boost/filesystem.hpp>

// This is likely just a temporary helper function to make creation of
// the cry device simpler.

//namespace cryfs { class CryDevice; }
namespace fspp { class Device; }

namespace ouisync {

std::unique_ptr<fspp::Device> create_cry_device(const fs::path& root);

} // namespace
