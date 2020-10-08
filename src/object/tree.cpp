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
