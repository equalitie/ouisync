#pragma once


#include "../user_id.h"
#include "../object_id.h"
#include "../object_tag.h"
#include "../shortcuts.h"
#include "../version_vector.h"
#include "../ouisync_assert.h"

#include <map>
#include <set>
#include <iosfwd>
#include <boost/filesystem/path.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>
#include <boost/range/adaptor/map.hpp>

namespace ouisync::object {

class Tree final {
public:
    struct VersionedObject {
        ObjectId object_id;
        VersionVector version_vector;

        template<class Archive>
        void serialize(Archive& ar, const unsigned int version) {
            ar & object_id & version_vector;
        }
    };

    using UserMap = std::map<UserId, VersionedObject>;

private:
    using NameMap = std::map<std::string, UserMap>;

    template<bool is_mutable>
    class Handle {
        using ImplPtr = std::conditional_t<is_mutable, NameMap*, const NameMap*>;
        using NameIterator = std::conditional_t<is_mutable, NameMap::iterator, NameMap::const_iterator>;
        using UserIterator = std::conditional_t<is_mutable, UserMap::iterator, UserMap::const_iterator>;

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

        const UserMap& versioned_ids() const {
            assert(name_map);
            if (!name_map) exit(1);
            return i->second;
        }

        UserMap& versioned_ids() {
            assert(name_map);
            if (!name_map) exit(1);
            return i->second;
        }

        operator bool() const { return name_map != nullptr; }

        UserIterator begin() const { if (name_map) return i->second.begin(); else return empty.begin(); }
        UserIterator end()   const { if (name_map) return i->second.end();   else return empty.end();   }

        UserIterator begin() { if (name_map) return i->second.begin(); else return empty.begin(); }
        UserIterator end()   { if (name_map) return i->second.end();   else return empty.end();   }

        UserIterator find(const UserId& uid) {
            ouisync_assert(name_map);
            return i->second.find(uid);
        }

      private:
        friend class Tree;
        Handle(ImplPtr name_map, NameIterator i)
            : name_map(name_map), i(i) {}

      private:
        ImplPtr name_map = nullptr;
        NameIterator i;
        UserMap empty;
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

    UserMap& operator[](const std::string& dirname) {
        return _name_map[dirname];
    }

    void erase(const MutableHandle& h)
    {
        assert(h.name_map);
        assert(h.name_map == &_name_map);
        if (h.name_map != &_name_map) exit(1);
        _name_map.erase(h.i);
    }

    auto children() const {
        std::set<ObjectId> ch;
        for (auto& [_, user_map] : _name_map) {
            for (auto& [user, vobj] : user_map) {
                ch.insert(vobj.object_id);
            }
        }
        return ch;
    }

    auto children_names() const {
        return boost::adaptors::keys(_name_map);
    }

    void print(std::ostream&, unsigned level = 0) const;

    VersionVector calculate_version_vector_union() const;

    template<class F>
    void for_each_unique_child(const F& f) const {
        for (auto& [filename, usermap] : _name_map) {
            std::set<ObjectId> used;
            for (auto& [user_id, vobj] : usermap) {
                if (used.count(vobj.object_id)) continue;
                used.insert(vobj.object_id);
                f(filename, vobj.object_id);
            }
        }
    }

public:
    static constexpr ObjectTag tag = ObjectTag::Tree;

    struct Nothing {
        static constexpr ObjectTag tag = Tree::tag;

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

