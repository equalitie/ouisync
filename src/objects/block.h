#pragma once

#include "../hash.h"
#include "../namespaces.h"

#include <vector>
#include <boost/serialization/vector.hpp>

namespace ouisync::objects {

class Block {
public:
    Sha256::Digest calculate_digest() const {
        Sha256 hash;
        hash.update(_data.data(), _data.size());
        return hash.close();
    }

    template<class Archive>
    void serialize(Archive& ar, const unsigned int version) {
        ar & _data;
    }

    Opt<Sha256::Digest> store(const fs::path&) const;

private:
    std::vector<uint8_t> _data;
};

} // namespace
