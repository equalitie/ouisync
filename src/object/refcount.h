#pragma once

#include "../object_id.h"
#include "../shortcuts.h"

#include <cstdint>

namespace ouisync::object::refcount {

using Number = uint32_t;

Number read(const fs::path&);
Number increment(const fs::path&);
Number decrement(const fs::path&);

Number read(const fs::path& objdir, const ObjectId&);
Number increment(const fs::path& objdir, const ObjectId&);
Number decrement(const fs::path& objdir, const ObjectId&);

} // namespace
