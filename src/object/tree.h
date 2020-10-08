#pragma once
#include "id.h"
#include "../namespaces.h"

#include <map>
#include <boost/filesystem/path.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>

namespace ouisync::object {

namespace {
    using TreeMap = std::map<std::string, Id>;
}

class Tree final : TreeMap {
public:
    Id calculate_id() const;
    Id store(const fs::path& root) const;

    template<class Archive>
    void serialize(Archive& ar, const unsigned int version) {
        ar & static_cast<TreeMap&>(*this);
    }
};

} // namespace

