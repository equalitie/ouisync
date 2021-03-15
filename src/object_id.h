#pragma once

#include "hash.h"

namespace ouisync {

class ObjectId : public Sha256::Digest {
private:
    using Parent = Sha256::Digest;
    using NameMap = std::map<ObjectId, std::list<std::string>>;

public:
    class RiaaNameMap {
      public:
        ~RiaaNameMap();

      private:
        friend class ObjectId;

        using MapIter  = NameMap::iterator;
        using ListIter = std::list<std::string>::iterator;

        RiaaNameMap(MapIter i, ListIter j) : map_iter(i), list_iter(j) {}


        MapIter map_iter;
        ListIter list_iter;
    };

public:
    static constexpr size_t size = std::tuple_size<Parent>::value;

    struct Hex : std::array<char, size*2>{
        static constexpr size_t size = ObjectId::size*2;
        using Parent = std::array<char, size>;
        Hex() = default;
        Hex(const Parent& p) : Parent(p) {}
        friend std::ostream& operator<<(std::ostream&, const Hex&);
    };

    struct ShortHex : std::array<char, std::min<size_t>(6, size*2)>{
        static constexpr size_t size = std::min<size_t>(6, ObjectId::size*2);
        using Parent = std::array<char, size>;
        ShortHex() = default;
        ShortHex(const Parent& p) : Parent(p) {}
        friend std::ostream& operator<<(std::ostream&, const ShortHex&);
    };

    static RiaaNameMap debug_name(const ObjectId&, std::string name);

public:
    using Parent::Parent;

    // XXX: Why isn't the above `using` enough?
    ObjectId(const Parent& p) : Parent(p) {}

    template<class Archive>
    void serialize(Archive& ar, unsigned)
    {
        ar & static_cast<Parent&>(*this);
    }

    static ObjectId null_id();

    void from_bytes(const char* bytes) {
        std::copy_n(bytes, size, Parent::data());
    }

    void to_bytes(char* bytes) const {
        std::copy_n(Parent::data(), size, bytes);
    }

    Hex hex() const;
    ShortHex short_hex() const;

    friend std::ostream& operator<<(std::ostream& os, const ObjectId&);
};

template<class Hash>
inline void update_hash(const ObjectId& id, Hash& hash)
{
    hash.update(static_cast<const typename Hash::Digest&>(id));
}

} // namespaces

namespace std {
    // For use with std::unordered_{map,set}
    template<> struct hash<ouisync::ObjectId> {
        size_t operator()(const ouisync::ObjectId& id) const {
            // It's already a Sha256 digest, so just return the first
            // sizeof(size_t) bytes.
            return *((size_t*) id.data());
        }
    };
} // std namespace
