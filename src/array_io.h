#include <iostream>
#include <array>

namespace ouisync {

    template<class T, size_t N>
    std::ostream& operator<<(std::ostream& os, const std::array<T, N>& as)
    {
        for (auto a : as) {
            os << a;
        }
        return os;
    }
}
