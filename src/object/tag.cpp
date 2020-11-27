#include "tag.h"

#include <ostream>

using namespace ouisync::object;

std::ostream& ouisync::object::operator<<(std::ostream& os, Tag tag) {
    switch (tag) {
        case Tag::Tree:     return os << "Tree";
        case Tag::Blob:     return os << "Blob";
        case Tag::ObjectId: return os << "ObjectId";
    }
    return os << "Unknown";
}
