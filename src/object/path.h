#pragma once

#include "id.h"
#include "../namespaces.h"

#include <boost/optional.hpp>
#include <boost/filesystem/path.hpp>

namespace ouisync::object::path {

fs::path from_id(const Id&);
Opt<Id>  to_id(const fs::path&);

} // namespaces
