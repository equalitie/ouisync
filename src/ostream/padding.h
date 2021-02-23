#pragma once

#include <ostream>

namespace ouisync {

    struct Padding {
        unsigned level = 0;
        Padding(unsigned level) : level(level) {}

        friend std::ostream& operator<<(std::ostream& os, Padding pad) {
            for (unsigned i = 0; i < pad.level; ++i) { os << " "; }
            return os;
        }
    };

} // namespace
