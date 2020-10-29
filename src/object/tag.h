#pragma once

#include "id.h"
#include "block.h"

#include <cstdint> // uint8_t
#include <iosfwd>

namespace ouisync::object {

enum class Tag : std::uint8_t {
    Tree = 1,
    Block,
    Id
};

/*
 * Use JustTag class template to probe whether the given object is of
 * expected_tag while avoiding reading the object from a file.
 * E.g.:
 *
 * if (maybe_load<JustTag<Data>>(objdir, object_id)) {
 *     remove(objdir, object_id);
 * }
 */
template<class T>
struct JustTag {
    template<class Archive>
    void serialize(Archive&, const unsigned int version) {}
};

class Tree;

template<class> struct GetTag;
template<> struct GetTag<Tree>  { static constexpr Tag value = Tag::Tree;  };
template<> struct GetTag<Block> { static constexpr Tag value = Tag::Block; };
template<> struct GetTag<Id>    { static constexpr Tag value = Tag::Id;    };
template<class T> struct GetTag<JustTag<T>> { static constexpr Tag value = GetTag<T>::value; };

std::ostream& operator<<(std::ostream& os, Tag tag);

} // namespaces
