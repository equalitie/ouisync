#include "version_vector.h"
#include "ostream/map.h"

using namespace ouisync;

std::ostream& ouisync::operator<<(std::ostream& os, const VersionVector& v)
{
    os << "{";
    bool is_first = true;
    for (auto& [k, v] : v._vv) {
        if (!is_first) os << ", ";
        is_first = false;
        os << k << ":" << v;
    }
    return os << "}";
}
