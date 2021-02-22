#include "versioned_object.h"

using namespace ouisync;

std::ostream& ouisync::operator<<(std::ostream& os, const VersionedObject& c) {
    return os << "VersionedObject{" << c.id << ", " << c.versions << "}";
}
