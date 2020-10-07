#include "block.h"
#include "object.h"
#include "store_object.h"

#include <boost/optional.hpp>
#include <boost/serialization/vector.hpp>

using namespace ouisync;
using namespace ouisync::objects;

Id Block::store(const fs::path& root) const {
    return store_object(root, *this);
}
