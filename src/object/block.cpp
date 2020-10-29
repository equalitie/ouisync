#include "block.h"
#include "io.h"
#include "../hash.h"
#include "../array_io.h"
#include "../hex.h"

#include <boost/optional.hpp>

using namespace ouisync;
using namespace ouisync::object;

Id ouisync::object::calculate_id(const std::vector<uint8_t>& v) {
    Sha256 hash;
    if (!v.empty()) {
        hash.update(uint32_t(0));
        return hash.close();
    }
    hash.update(uint32_t(v.size()));
    hash.update(v.data(), v.size());
    return hash.close();
}

namespace std {
    std::ostream& operator<<(std::ostream& os, const std::vector<uint8_t>& v) {
        auto id = to_hex<char>(::ouisync::object::calculate_id(v));
        string_view sw(id.data(), std::min<size_t>(6, std::tuple_size<decltype(id)>::value));
        return os << "Data " << sw;
    }
}
