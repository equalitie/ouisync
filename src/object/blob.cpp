#include "blob.h"
#include "io.h"
#include "../hash.h"
#include "../array_io.h"
#include "../hex.h"

#include <boost/optional.hpp>

using namespace ouisync;
using namespace ouisync::object;

Id ouisync::object::calculate_id(const Blob& v) {
    Sha256 hash;
    if (v.empty()) {
        hash.update(uint32_t(0));
        return hash.close();
    }
    hash.update(uint32_t(v.size()));
    hash.update(v.data(), v.size());
    return hash.close();
}

namespace std {
    std::ostream& operator<<(std::ostream& os, const Blob& v) {
        auto id = ::ouisync::object::calculate_id(v);
        return os << "Data id:" << id << " size:" << v.size();
    }
}
