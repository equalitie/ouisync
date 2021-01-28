#pragma once

#include <ostream>
#include <map>

namespace std {

template<class K, class V>
inline
std::ostream& operator<<(std::ostream& os, const std::map<K, V>& m) {
    os << "{";
    bool is_first = true;
    for (auto& [k, v] : m) {
        if (!is_first) os << ", ";
        is_first = false;
        os << "{" << k << ", " << v << "}";
    }
    return os << "}";
}

} // namespace
