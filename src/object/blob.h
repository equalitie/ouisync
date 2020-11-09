#pragma once

#include "id.h"
#include "../shortcuts.h"

namespace ouisync::object {

struct Blob : std::vector<uint8_t>
{
    using Parent = std::vector<uint8_t>;
    using std::vector<uint8_t>::vector;

    Blob(const Parent& p) : Parent(p) {}
    Blob(Parent&& p) : Parent(std::move(p)) {}

    template<class Archive>
    void serialize(Archive& ar, const unsigned int version) {
        ar & static_cast<std::vector<uint8_t>&>(*this);
    }
};

Id calculate_id(const Blob&);

} // namespace

namespace std {
    std::ostream& operator<<(std::ostream&, const ::ouisync::object::Blob&);
}
