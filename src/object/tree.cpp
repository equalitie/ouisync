#include "tree.h"
#include "tagged.h"
#include "io.h"

#include <iostream>
#include <boost/filesystem.hpp>

using namespace ouisync;
using namespace ouisync::object;

Id ouisync::object::calculate_id(const Tree& tree)
{
    Sha256 hash;
    hash.update(uint32_t(hash.size()));
    for (auto& [k,v] : tree) {
        hash.update(k);
        hash.update(v);
    }
    return hash.close();
}

Id Tree::store(const fs::path& root) const
{
    return object::io::store(root, *this);
}

std::ostream& ouisync::object::operator<<(std::ostream& os, const Tree& tree) {
    auto id = calculate_id(tree);
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
