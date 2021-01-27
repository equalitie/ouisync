#pragma once

#include "tag.h"

#include "../object_id.h"
#include "../shortcuts.h"
#include "../version_vector.h"

#include <map>
#include <set>
#include <iosfwd>
#include <boost/filesystem/path.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>

namespace ouisync::object {

class Tree final {
public:
    using ObjectVersions = std::vector<VersionVector>;
    using VersionedIds = std::map<ObjectId, ObjectVersions>;

private:
    using NameMap = std::map<std::string, VersionedIds>;
    //using NameMap = std::map<std::string, ObjectId>;

    template<bool is_mutable>
    class Handle {
        using ImplPtr = std::conditional_t<is_mutable, NameMap*, const NameMap*>;
        using NameIterator = std::conditional_t<is_mutable, NameMap::iterator, NameMap::const_iterator>;

      public:
        Handle() = default;
        Handle(const Handle&) = default;
        Handle& operator=(const Handle&) = default;

        Handle(Handle&& other) : name_map(other.name_map), i(other.i)
        {
            other.name_map = nullptr;
            other.i = NameMap::iterator();
        }

        Handle& operator=(Handle&& other)
        {
            name_map = other.name_map;
            i = other.i;
            other.name_map = nullptr;
            other.i = NameMap::iterator();
            return *this;
        }

        const VersionedIds& versioned_ids() const {
            assert(name_map);
            if (!name_map) exit(1);
            return i->second;
        }

        VersionedIds& versioned_ids() {
            assert(name_map);
            if (!name_map) exit(1);
            return i->second;
        }

        operator bool() const { return name_map != nullptr; }

        //void set_id(const ObjectId& id) {
        //    assert(name_map);
        //    if (!name_map) exit(1);
        //    if (id == i->second) return;
        //    i->second = id;
        //}

        auto begin() const { if (name_map) return i->second.begin(); else return empty.begin(); }
        auto end()   const { if (name_map) return i->second.end();   else return empty.end();   }

      private:
        friend class Tree;
        Handle(ImplPtr name_map, NameIterator i)
            : name_map(name_map), i(i) {}

      private:
        ImplPtr name_map = nullptr;
        NameIterator i;
        VersionedIds empty;
    };

public:
    auto begin() const { return _name_map.begin(); }
    auto end()   const { return _name_map.end();   }

    size_t size() const { return _name_map.size(); }

    using MutableHandle   = Handle<true>;
    using ImmutableHandle = Handle<false>;

    ImmutableHandle find(const std::string& k) const {
        auto i = _name_map.find(k);
        if (i == _name_map.end()) return {};
        return {&_name_map, i};
    }

    MutableHandle find(const std::string& k) {
        auto i = _name_map.find(k);
        if (i == _name_map.end()) return {};
        return {&_name_map, i};
    }

    void erase(const MutableHandle& h)
    {
        assert(h.name_map);
        assert(h.name_map == &_name_map);
        if (h.name_map != &_name_map) exit(1);
        _name_map.erase(h.i);
    }

    //std::pair<MutableHandle, bool>
    //insert(std::pair<std::string, ObjectId> pair) {
    //    auto [i,inserted] = _name_map.insert(std::move(pair));
    //    return {MutableHandle{&_name_map, i}, inserted};
    //}

    //MutableHandle operator[](std::string key) {
    //    //auto [i,inserted] = _name_map.insert({std::move(key), {}});
    //    //return MutableHandle{&_name_map, i};
    //    return insert({std::move(key), ObjectId{}}).first;
    //}

    auto children() const {
        std::set<ObjectId> ch;
        for (auto& [_, ids] : _name_map) {
            for (auto& [id, versions] : ids) {
                ch.insert(id);
            }
        }
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
        ar & _name_map;
    }

    ObjectId calculate_id() const;

    friend std::ostream& operator<<(std::ostream&, const Tree&);

private:
    NameMap _name_map;
};



} // namespace

