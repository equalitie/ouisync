#include "tree.h"
#include "tagged.h"
#include "object.h"
#include "store_object.h"

#include "../hex.h"
#include "../array_io.h"

#include <boost/archive/text_oarchive.hpp>
#include <iostream>
#include <boost/filesystem.hpp>

using namespace ouisync;
using namespace ouisync::objects;

Sha256::Digest Tree::calculate_digest() const
{
    Sha256 hash;
    for (auto& [k,v] : *this) {
        hash.update(k);
        hash.update(v);
    }
    return hash.close();
}

Opt<Sha256::Digest> Tree::store(const fs::path& root) const
{
    return store_object(root, *this);
}
