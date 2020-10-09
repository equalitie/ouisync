#include "tree.h"
#include "tagged.h"
#include "io.h"

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
    return object::io::store(root, *this);
}

std::ostream& ouisync::object::operator<<(std::ostream& os, const Tree& tree) {
    auto hex_id = to_hex<char>(tree.calculate_id());

    const auto short_sw = [](const auto& a) -> string_view {
        return string_view(a.data(),
               std::min<size_t>(6, std::tuple_size<std::decay_t<decltype(a)>>::value));
    };

    os << "Tree id:" << short_sw(hex_id) << " size:" << tree.size();

    for (auto& [k, v] : tree) {
        auto hex_id = to_hex<char>(v);
        os << " " << k << ":" << short_sw(hex_id);
    }

    return os;
}
