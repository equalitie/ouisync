#pragma once

#include "tag.h"

#include <boost/variant.hpp>
#include <boost/serialization/split_member.hpp>
#include <boost/archive/archive_exception.hpp>

namespace ouisync::object::tagged {

namespace detail {
    std::exception bad_tag_exception(Tag requested, Tag parsed);
}

template<class Obj>
struct Save {
    const Obj& obj;

    template<class Archive>
    void save(Archive& ar, const unsigned int version) const {
        ar & Obj::tag;
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

        if (tag != Obj::tag) {
            throw detail::bad_tag_exception(Obj::tag, tag);
        }

        ar & obj;
    }

    BOOST_SERIALIZATION_SPLIT_MEMBER()
};

namespace detail {
    template<class T, class... Ts> struct LoadVariant {
        template<class Variant, class Archive>
        static void load(Tag tag, Variant& result, Archive& ar) {
            if (tag == T::tag) {
                T obj;
                ar & obj;
                result = std::move(obj);
                return;
            }
            LoadVariant<Ts...>::load(tag, result, ar);
        }
    };
    template<class T> struct LoadVariant<T> {
        template<class Variant, class Archive>
        static void load(Tag tag, Variant& result, Archive& ar) {
            if (tag == T::tag) {
                T obj;
                ar & obj;
                result = std::move(obj);
                return;
            }
            throw boost::archive::archive_exception(
                    boost::archive::archive_exception::unregistered_class);
        }
    };
} // detail namespace

template<class... Ts>
struct Load<boost::variant<Ts...>> {
    using Variant = boost::variant<Ts...>;
    Variant& var;

    template<class Archive>
    void load(Archive& ar, const unsigned int version) {
        Tag tag;
        ar & tag;
        detail::LoadVariant<Ts...>::load(tag, var, ar);
    }

    BOOST_SERIALIZATION_SPLIT_MEMBER()
};

} // namespaces
