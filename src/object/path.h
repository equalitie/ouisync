#pragma once

#include "object_id.h"
#include "../shortcuts.h"

#include <boost/optional.hpp>
#include <boost/filesystem/path.hpp>

namespace ouisync::object::path {

fs::path from_id(const ObjectId&) noexcept;
Opt<ObjectId>  to_id(const fs::path&) noexcept;

} // namespaces
