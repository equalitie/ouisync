#pragma once

#include <cstdint> // uint8_t
#include <iosfwd>

namespace ouisync::object {

enum class Tag : std::uint8_t {
    Tree = 1,
    Blob,
    ObjectId
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
    static constexpr Tag tag = T::tag;

    template<class Archive>
    void serialize(Archive&, const unsigned int version) {}
};

std::ostream& operator<<(std::ostream& os, Tag tag);

} // namespaces
