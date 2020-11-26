#include "commit.h"

using namespace ouisync;

std::ostream& ouisync::operator<<(std::ostream& os, const Commit& c) {
    return os << "Commit{" << c.root_object_id << ", <version-vector>}";
}
