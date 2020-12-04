#pragma once

#include "object_id.h"
#include "shortcuts.h"

#include <cstdint>

namespace ouisync::refcount {

using Number = uint32_t;

Number read(const fs::path&);
Number increment(const fs::path&);
Number decrement(const fs::path&);

Number read(const fs::path& objdir, const ObjectId&);
Number increment(const fs::path& objdir, const ObjectId&);
Number decrement(const fs::path& objdir, const ObjectId&);

bool flat_remove(const fs::path& objdir, const ObjectId& id);
void deep_remove(const fs::path& objdir, const ObjectId& id);

} // namespace
