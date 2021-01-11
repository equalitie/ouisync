#pragma once

#include <ostream>
#include <set>

namespace std {

template<class T>
inline
std::ostream& operator<<(std::ostream& os, const std::set<T>& s) {
    os << "{";
    bool is_first = true;
    for (auto& e : s) {
        if (!is_first) os << ", ";
        is_first = false;
        os  << e;
    }
    return os << "}";
}

} // namespace
