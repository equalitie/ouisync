#include "object_tag.h"

#include <ostream>

using namespace ouisync;

std::ostream& ouisync::operator<<(std::ostream& os, ObjectTag tag) {
    switch (tag) {
        case ObjectTag::Tree:     return os << "Tree";
        case ObjectTag::Blob:     return os << "Blob";
    }
    return os << "Unknown";
}
