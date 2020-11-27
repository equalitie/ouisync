#include "tree.h"
#include "tagged.h"
#include "io.h"

#include <iostream>
#include <boost/filesystem.hpp>

using namespace ouisync;
using namespace ouisync::object;

ObjectId Tree::calculate_id() const
{
    Sha256 hash;
    hash.update(static_cast<std::underlying_type_t<Tag>>(tag));
    hash.update(uint32_t(size()));
    for (auto& [k,v] : *this) {
        hash.update(k);
        hash.update(v);
    }
    return hash.close();
}

std::ostream& ouisync::object::operator<<(std::ostream& os, const Tree& tree) {
    os << "Tree id:" << tree.calculate_id() << " [";

    bool is_first = true;
    for (auto& [k, v] : tree) {
        if (!is_first) os << ", ";
        is_first = false;
        os << k << ":" << v;
    }

    return os << "]";
}
