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
    hash.update(uint32_t(hash.size()));
    for (auto& [k,v] : *this) {
        hash.update(k);
        hash.update(v);
    }
    return hash.close();
}

ObjectId Tree::store(const fs::path& root) const
{
    return object::io::store(root, *this);
}

std::ostream& ouisync::object::operator<<(std::ostream& os, const Tree& tree) {
    auto id = tree.calculate_id();
    auto hex_id = id.short_hex();

    os << "Tree id:" << hex_id << " [";

    bool is_first = true;
    for (auto& [k, v] : tree) {
        if (!is_first) os << ", ";
        is_first = false;
        os << k << ":" << v.short_hex();
    }

    return os << "]";
}
