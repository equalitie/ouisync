#pragma once

#include "../hash.h"

namespace ouisync::object {

class Id : public Sha256::Digest {
private:
    using Parent = Sha256::Digest;

public:
    static constexpr size_t size = std::tuple_size<Parent>::value;

    struct Hex : std::array<char, size*2>{
        static constexpr size_t size = Id::size*2;
        using Parent = std::array<char, size>;
        Hex() = default;
        Hex(const Parent& p) : Parent(p) {}
        friend std::ostream& operator<<(std::ostream&, const Hex&);
    };

    struct ShortHex : std::array<char, std::min<size_t>(6, size*2)>{
        static constexpr size_t size = std::min<size_t>(6, Id::size*2);
        using Parent = std::array<char, size>;
        ShortHex() = default;
        ShortHex(const Parent& p) : Parent(p) {}
        friend std::ostream& operator<<(std::ostream&, const ShortHex&);
    };

public:
    using Parent::Parent;

    // XXX: Why isn't the above `using` enough?
    Id(const Parent& p) : Parent(p) {}

    template<class Archive>
    void serialize(Archive& ar, unsigned)
    {
        ar & static_cast<Parent&>(*this);
    }

    Hex hex() const;
    ShortHex short_hex() const;

    friend std::ostream& operator<<(std::ostream& os, const Id&);
};

} // namespaces
