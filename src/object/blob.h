#pragma once

#include "id.h"
#include "../shortcuts.h"

namespace ouisync::object {

using Blob = std::vector<uint8_t>;

Id calculate_id(const Blob&);

} // namespace

namespace std {
    std::ostream& operator<<(std::ostream&, const ::ouisync::object::Blob&);
}
