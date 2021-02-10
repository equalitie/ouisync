#include "file_blob.h"
#include "hash.h"

#include <ostream>
#include <boost/optional.hpp>

using namespace ouisync;

ObjectId FileBlob::calculate_id() const {
    Sha256 hash;
    hash.update(static_cast<std::underlying_type_t<ObjectTag>>(tag));
    hash.update(uint32_t(size()));
    hash.update(data(), size());
    return hash.close();
}

std::ostream& ouisync::operator<<(std::ostream& os, const FileBlob& b) {
    auto id = b.calculate_id();
    return os << "Data id:" << id << " size:" << b.size();
}
