#include "block.h"
#include "object.h"
#include "store.h"

#include <boost/optional.hpp>

using namespace ouisync;
using namespace ouisync::objects;

Id Block::store(const fs::path& root) const {
    return objects::store(root, *this);
}
