#pragma once

#include "id.h"
#include "../namespaces.h"
#include "../variant.h"

#include "cpp-utils/data/Data.h"
#include <boost/serialization/array_wrapper.hpp>

namespace ouisync::object {

class Block {
private:
    using Data = cpputils::Data;
    // Reference: because data may come from above
    // Optional: because Data is not default constructible and we
    // don't know the size up front (to be able to construct it).
    using RefOrOptData = boost::variant<
        std::reference_wrapper<Data>,
        Opt<Data>>;

public:
    Block() : _data(boost::none) {}
    Block(Data& data) : _data(std::ref(data)) {}
    // XXX: Can we avoid doing const_cast while also not creating whole
    // new Block class for holding const references? As adding new class
    // would cause another entry in Object::Variant.
    Block(const Data& data) : _data(std::ref(const_cast<Data&>(data))) {}

    Id calculate_id() const;

    template<class Archive>
    void save(Archive& ar, const unsigned int version) const {
        auto d = data();
        auto ptr = static_cast<const uint8_t*>(d ? d->data() : nullptr);
        uint32_t size = d ? d->size() : 0;
        ar & size;
        ar & boost::serialization::make_array<const uint8_t>(ptr, size);
    }

    template<class Archive>
    void load(Archive& ar, const unsigned int version) {
        uint32_t size;
        ar & size;
        auto d = data();
        if (!d || d->size() != size) {
            _data = Opt<Data>(Data(size));
            d = data();
        }
        auto ptr = static_cast<uint8_t*>(d->data());
        ar & boost::serialization::make_array<uint8_t>(ptr, size);
    }

    Id store(const fs::path&) const;

          Data* data();
    const Data* data() const;

    BOOST_SERIALIZATION_SPLIT_MEMBER()

private:
    RefOrOptData _data;
};

std::ostream& operator<<(std::ostream&, const Block&);

} // namespace
