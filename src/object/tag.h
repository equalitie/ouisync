#pragma once

#include "id.h"

#include <cstdint> // uint8_t
#include <iosfwd>

namespace ouisync::object {

enum class Tag : std::uint8_t {
    Tree = 1,
    Block,
    Id
};

class Tree;
class Block;

template<class> struct GetTag;
template<> struct GetTag<Tree>  { static constexpr Tag value = Tag::Tree;  };
template<> struct GetTag<Block> { static constexpr Tag value = Tag::Block; };
template<> struct GetTag<Id>    { static constexpr Tag value = Tag::Id;    };

std::ostream& operator<<(std::ostream& os, Tag tag);

} // namespaces
