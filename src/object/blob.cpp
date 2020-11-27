#include "blob.h"
#include "io.h"
#include "../hash.h"
#include "../array_io.h"
#include "../hex.h"

#include <boost/optional.hpp>

using namespace ouisync;
using namespace ouisync::object;

ObjectId Blob::calculate_id() const {
    Sha256 hash;
    hash.update(uint32_t(size()));
    hash.update(data(), size());
    return hash.close();
}

std::ostream& ouisync::object::operator<<(std::ostream& os, const Blob& b) {
    auto id = b.calculate_id();
    return os << "Data id:" << id << " size:" << b.size();
}
