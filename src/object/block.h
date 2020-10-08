#pragma once

#include "id.h"
#include "../hash.h"
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

    Id calculate_id() const {
        Sha256 hash;
        const Data* d = data();
        if (!d) return hash.close();
        hash.update(d->data(), d->size());
        return hash.close();
    }

    template<class Archive>
    void serialize(Archive& ar, const unsigned int version) {
        auto d = data();
        auto ptr = static_cast<uint8_t*>(d ? d->data() : nullptr);
        auto size = d ? d->size() : 0;
        ar & boost::serialization::make_array<uint8_t>(ptr, size);
    }

    Id store(const fs::path&) const;

    Data* data() {
        return apply(_data,
                [](std::reference_wrapper<Data> d) {
                    return &d.get();
                },
                [](Opt<Data>& d) -> Data* {
                    if (!d) return nullptr;
                    return &*d;
                });
    }

    const Data* data() const {
        return apply(_data,
                [](const std::reference_wrapper<Data> d) {
                    return &d.get();
                },
                [](const Opt<Data>& d) -> const Data* {
                    if (!d) return nullptr;
                    return &*d;
                });
    }

private:
    RefOrOptData _data;
};

} // namespace
