#pragma once

#include "tag.h"
#include "object.h"

#include <boost/archive/archive_exception.hpp>

namespace ouisync::objects::tagged {

template<class Obj>
struct Save {
    const Obj& obj;

    template<class Archive>
    void save(Archive& ar, const unsigned int version) const {
        ar & GetTag<Obj>::value;
        ar & obj;
    }

    BOOST_SERIALIZATION_SPLIT_MEMBER()
};

template<class Obj>
struct Load {
    Obj& obj;

    template<class Archive>
    void load(Archive& ar, const unsigned int version) {
        Tag tag;
        ar & tag;

        if (tag != GetTag<Obj>::value) {
            throw std::runtime_error("Object not of the requested type");
        }

        ar & obj;
    }

    BOOST_SERIALIZATION_SPLIT_MEMBER()
};

template<>
struct Load<Object::Variant> {
    Object::Variant& var;

    template<class Archive>
    void load(Archive& ar, const unsigned int version) {
        Tag tag;
        ar & tag;

        switch (tag) {
            case Tag::Tree: { 
                Tree t;
                ar & t;
                var = std::move(t);
                return;
            }
            case Tag::Block: {
                Block b;
                ar & b;
                var = std::move(b);
                return;
            }
        }

        throw boost::archive::archive_exception(
                boost::archive::archive_exception::unregistered_class);
    }

    BOOST_SERIALIZATION_SPLIT_MEMBER()
};

} // namespaces
