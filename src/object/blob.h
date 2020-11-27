#pragma once

#include "tag.h"
#include "../object_id.h"
#include "../shortcuts.h"

namespace ouisync::object {

struct Blob : std::vector<uint8_t>
{
    static constexpr Tag tag = Tag::Blob;

    using Parent = std::vector<uint8_t>;
    using std::vector<uint8_t>::vector;

    Blob(const Parent& p) : Parent(p) {}
    Blob(Parent&& p) : Parent(std::move(p)) {}

    ObjectId calculate_id() const;

    template<class Archive>
    void serialize(Archive& ar, const unsigned int version) {
        ar & static_cast<std::vector<uint8_t>&>(*this);
    }

    friend std::ostream& operator<<(std::ostream&, const Blob&);
};

} // namespace

