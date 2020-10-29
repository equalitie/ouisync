#pragma once

#include "id.h"
#include "../shortcuts.h"

namespace ouisync::object {

using Block = std::vector<uint8_t>;

Id calculate_id(const Block&);

} // namespace

namespace std {
    std::ostream& operator<<(std::ostream&, const ::ouisync::object::Block&);
}
