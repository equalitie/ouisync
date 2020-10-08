#include "tree.h"
#include "tagged.h"
#include "store.h"

#include "../hex.h"
#include "../array_io.h"

#include <iostream>
#include <boost/filesystem.hpp>

using namespace ouisync;
using namespace ouisync::object;

Id Tree::calculate_id() const
{
    Sha256 hash;
    for (auto& [k,v] : *this) {
        hash.update(k);
        hash.update(v);
    }
    return hash.close();
}

Id Tree::store(const fs::path& root) const
{
    return object::store(root, *this);
}

std::ostream& ouisync::object::operator<<(std::ostream& os, const Tree& tree) {
    os << "Tree " << tree.size();
    for (auto& [k, v] : tree) {
        auto hex = to_hex<char>(v);
        string_view short_hex(hex.data(),
               std::min<size_t>(6, std::tuple_size<decltype(hex)>::value));

        os << " " << k << ":" << short_hex;
    }
    return os;
}
