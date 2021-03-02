#include "object_tag.h"

#include <ostream>

using namespace ouisync;

std::ostream& ouisync::operator<<(std::ostream& os, ObjectTag tag) {
    switch (tag) {
        case ObjectTag::Directory: return os << "Directory";
        case ObjectTag::File:      return os << "File";
    }
    return os << "Unknown";
}
