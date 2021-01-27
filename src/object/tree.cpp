#include "tree.h"
#include "tagged.h"

#include <iostream>
#include <boost/filesystem.hpp>

using namespace ouisync;
using namespace ouisync::object;

static void _hash(Sha256 hash, const Tree::VersionedIds& ids) {
    for (auto& [id, vv] : ids) {
        hash.update(id);
        hash.update(uint32_t(vv.size()));
        assert("TODO" && 0);
        //for (auto& [a,b] : vv) {
        //    hash.update(a);
        //    hash.update(b);
        //}
    }
}

ObjectId Tree::calculate_id() const
{
    Sha256 hash;
    hash.update(static_cast<std::underlying_type_t<Tag>>(tag));
    hash.update(uint32_t(size()));
    for (auto& [k,v] : *this) {
        hash.update(k);
        _hash(hash, v);
    }
    return hash.close();
}

std::ostream& ouisync::object::operator<<(std::ostream& os, const Tree& tree) {
    os << "Tree id:" << tree.calculate_id() << " [";

    //bool is_first = true;
    //for (auto& [k, v] : tree) {
    //    if (!is_first) os << ", ";
    //    is_first = false;
    //    os << k << ":" << v;
    //}

    return os << "]";
}
