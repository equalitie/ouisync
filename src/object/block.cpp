#include "block.h"
#include "store.h"

#include <boost/optional.hpp>

using namespace ouisync;
using namespace ouisync::object;

Id Block::store(const fs::path& root) const {
    return object::store(root, *this);
}
