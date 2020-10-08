#pragma once
#include "id.h"
#include "../namespaces.h"

#include <map>
#include <boost/filesystem/path.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>

namespace ouisync::object {

namespace {
    using TreeMap = std::map<std::string, Sha256::Digest>;
}

class Tree final : TreeMap {
public:
    Sha256::Digest calculate_digest() const;

    template<class Archive>
    void serialize(Archive& ar, const unsigned int version) {
        ar & static_cast<TreeMap&>(*this);
    }

    Id store(const fs::path& root) const;
};

} // namespace

