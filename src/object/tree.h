#pragma once

#include "tag.h"

#include "../object_id.h"
#include "../shortcuts.h"

#include <map>
#include <set>
#include <iosfwd>
#include <boost/filesystem/path.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>

namespace ouisync::object {

class Tree final {
private:
    using Impl = std::map<std::string, ObjectId>;

    template<bool is_mutable>
    class Handle {
        using ImplPtr = std::conditional_t<is_mutable, Impl*, const Impl*>;
        using Iterator = std::conditional_t<is_mutable, Impl::iterator, Impl::const_iterator>;

      public:
        Handle() = default;
        Handle(const Handle&) = default;
        Handle& operator=(const Handle&) = default;

        Handle(Handle&& other) : impl(other.impl), i(other.i)
        {
            other.impl = nullptr;
            other.i = Impl::iterator();
        }

        Handle& operator=(Handle&& other)
        {
            impl = other.impl;
            i = other.i;
            other.impl = nullptr;
            other.i = Impl::iterator();
            return *this;
        }

        ObjectId id() const {
            assert(impl);
            if (!impl) exit(1);
            return i->second;
        }

        operator bool() const { return impl != nullptr; }

        void set_id(const ObjectId& id) {
            assert(impl);
            if (!impl) exit(1);
            if (id == i->second) return;
            i->second = id;
        }

      private:
        friend class Tree;
        Handle(ImplPtr impl, Iterator i)
            : impl(impl), i(i) {}

      private:
        ImplPtr impl = nullptr;
        Iterator i;
    };

public:
    auto begin() const { return _impl.begin(); }
    auto end()   const { return _impl.end();   }

    size_t size() const { return _impl.size(); }

    using MutableHandle   = Handle<true>;
    using ImmutableHandle = Handle<false>;

    ImmutableHandle find(const std::string& k) const {
        auto i = _impl.find(k);
        if (i == _impl.end()) return {};
        return {&_impl, i};
    }

    MutableHandle find(const std::string& k) {
        auto i = _impl.find(k);
        if (i == _impl.end()) return {};
        return {&_impl, i};
    }

    void erase(const MutableHandle& h)
    {
        assert(h.impl);
        assert(h.impl == &_impl);
        if (h.impl != &_impl) exit(1);
        _impl.erase(h.i);
    }

    std::pair<MutableHandle, bool>
    insert(std::pair<std::string, ObjectId> pair) {
        auto [i,inserted] = _impl.insert(std::move(pair));
        return {MutableHandle{&_impl, i}, inserted};
    }

    MutableHandle operator[](std::string key) {
        return insert({std::move(key), ObjectId{}}).first;
    }

    std::set<ObjectId> children() const {
        std::set<ObjectId> ch;
        for (auto& [_, id] : _impl) { ch.insert(id); }
        return ch;
    }

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
        ar & _impl;
    }

    ObjectId calculate_id() const;

    friend std::ostream& operator<<(std::ostream&, const Tree&);

private:
    Impl _impl;
};



} // namespace

