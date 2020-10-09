#include "block.h"
#include "io.h"
#include "../hash.h"
#include "../array_io.h"
#include "../hex.h"

#include <boost/optional.hpp>

using namespace ouisync;
using namespace ouisync::object;

Id Block::calculate_id() const {
    Sha256 hash;
    const Data* d = data();
    if (!d) return hash.close();
    hash.update(d->data(), d->size());
    return hash.close();
}

Id Block::store(const fs::path& root) const {
    return object::store(root, *this);
}

std::ostream& ouisync::object::operator<<(std::ostream& os, const Block& block) {
    auto id = to_hex<char>(block.calculate_id());
    string_view sw(id.data(), std::min<size_t>(6, std::tuple_size<decltype(id)>::value));
    return os << "Block " << sw;
}

Block::Data* Block::data() {
    return apply(_data,
            [](std::reference_wrapper<Data> d) {
                return &d.get();
            },
            [](Opt<Data>& d) -> Data* {
                if (!d) return nullptr;
                return &*d;
            });
}

const Block::Data* Block::data() const {
    return apply(_data,
            [](const std::reference_wrapper<Data> d) {
                return &d.get();
            },
            [](const Opt<Data>& d) -> const Data* {
                if (!d) return nullptr;
                return &*d;
            });
}
