#pragma once

#include "id.h"
#include "../shortcuts.h"

#include <cstdint>

namespace ouisync::object::refcount {

using Number = uint32_t;

Number read(const fs::path& objdir, const Id&);
Number increment(const fs::path& objdir, const Id&);
Number decrement(const fs::path& objdir, const Id&);

} // namespace
