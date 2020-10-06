#pragma once

namespace ouisync::objects {

enum class Tag : uint8_t {
    Tree = 1,
    Block
};

class Tree;
class Block;
template<class> struct GetTag;
template<> struct GetTag<Tree>  { static constexpr Tag value = Tag::Tree; };
template<> struct GetTag<Block> { static constexpr Tag value = Tag::Block; };

} // namespaces
