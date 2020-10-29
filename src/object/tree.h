#pragma once
#include "id.h"
#include "../shortcuts.h"

#include <map>
#include <iosfwd>
#include <boost/filesystem/path.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>

namespace ouisync::object {

namespace {
    using TreeMap = std::map<std::string, Id>;
}

class Tree final : public TreeMap {
public:
    Id store(const fs::path& root) const;

    template<class Archive>
    void serialize(Archive& ar, const unsigned int version) {
        ar & static_cast<TreeMap&>(*this);
    }
};

Id calculate_id(const Tree&);

std::ostream& operator<<(std::ostream&, const Tree&);

} // namespace

