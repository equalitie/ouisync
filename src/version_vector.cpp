#include "version_vector.h"
#include "ostream/map.h"

using namespace ouisync;

std::ostream& ouisync::operator<<(std::ostream& os, const VersionVector& v)
{
    return os << v._vv;
}
