#include "object_tag.h"

#include <ostream>

using namespace ouisync;

std::ostream& ouisync::operator<<(std::ostream& os, ObjectTag tag) {
    switch (tag) {
        case ObjectTag::Directory: return os << "Tree";
        case ObjectTag::FileBlob:  return os << "FileBlob";
    }
    return os << "Unknown";
}
