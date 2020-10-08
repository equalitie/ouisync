#include "block.h"
#include "store.h"
#include "../array_io.h"
#include "../hex.h"

#include <boost/optional.hpp>

using namespace ouisync;
using namespace ouisync::object;

Id Block::store(const fs::path& root) const {
    return object::store(root, *this);
}

std::ostream& ouisync::object::operator<<(std::ostream& os, const Block& block) {
    auto id = to_hex<char>(block.calculate_id());
    string_view sw(id.data(), std::min<size_t>(6, std::tuple_size<decltype(id)>::value));
    return os << "Block " << sw;
}
