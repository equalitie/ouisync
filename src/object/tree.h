#pragma once

#include "tag.h"

#include "../object_id.h"
#include "../shortcuts.h"

#include <map>
#include <iosfwd>
#include <boost/filesystem/path.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>

namespace ouisync::object {

namespace {
    using TreeMap = std::map<std::string, ObjectId>;
}

class Tree final : public TreeMap {
public:
    static constexpr Tag tag = Tag::Tree;

    struct Nothing {
        static constexpr Tag tag = Tree::tag;

        template<class Archive>
        void load(Archive& ar, const unsigned int version) {}

        BOOST_SERIALIZATION_SPLIT_MEMBER()
    };

    template<class Archive>
    void serialize(Archive& ar, const unsigned int version) {
        ar & static_cast<TreeMap&>(*this);
    }

    ObjectId calculate_id() const;

    friend std::ostream& operator<<(std::ostream&, const Tree&);
};



} // namespace

