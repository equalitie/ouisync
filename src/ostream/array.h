#pragma once

#include <ostream>
#include <array>

namespace std {

template<class T, size_t N>
inline
std::ostream& operator<<(std::ostream& os, const std::array<T, N>& as)
{
    for (auto a : as) {
        os << a;
    }
    return os;
}

} // namespace
