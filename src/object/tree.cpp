#include "tree.h"
#include "tagged.h"
#include "io.h"

#include "../hex.h"
#include "../array_io.h"

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
    auto hex_id = calculate_id(tree).short_hex();

    os << "Tree id:" << hex_id << " size:" << tree.size();

    for (auto& [k, v] : tree) {
        os << " " << k << ":" << v.short_hex();
    }

    return os;
}
