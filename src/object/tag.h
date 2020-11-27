#pragma once

#include <cstdint> // std::uint8_t
#include <iosfwd>

namespace ouisync::object {

// Note: the underlying type contributes to object_id calculation, thus
// changing it will break the object storage as well as the peer exchange
// protocol.
enum class Tag : std::uint8_t {
    Tree = 1,
    Blob,
    ObjectId
};

std::ostream& operator<<(std::ostream& os, Tag tag);

} // namespaces
