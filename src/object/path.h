#pragma once

#include "id.h"
#include "../shortcuts.h"

#include <boost/optional.hpp>
#include <boost/filesystem/path.hpp>

namespace ouisync::object::path {

fs::path from_id(const Id&) noexcept;
Opt<Id>  to_id(const fs::path&) noexcept;

} // namespaces
