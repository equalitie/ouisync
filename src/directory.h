#pragma once

#include "user_id.h"
#include "object_tag.h"
#include "shortcuts.h"
#include "versioned_object.h"
#include "ouisync_assert.h"
#include "block_store.h"

#include <map>
#include <set>
#include <iosfwd>
#include <boost/filesystem/path.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/array.hpp>
#include <boost/range/adaptor/map.hpp>

namespace ouisync {

class Blob;
class Transaction;

class Directory final {
public:
    using UserMap = std::map<UserId, VersionedObject>;

private:
    // Need to use std::less<> to enable access using string_view.
    // https://stackoverflow.com/a/35525806/273348
    using NameMap = std::map<std::string, UserMap, std::less<>>;

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
        friend class Directory;
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

    ImmutableHandle find(const string_view k) const {
        auto i = _name_map.find(k);
        if (i == _name_map.end()) return {};
        return {&_name_map, i};
    }

    MutableHandle find(const string_view k) {
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
                ch.insert(vobj.id);
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
                if (used.count(vobj.id)) continue;
                used.insert(vobj.id);
                f(filename, vobj.id);
            }
        }
    }

public:
    ObjectId calculate_id() const;

    ObjectId save(BlockStore&, Transaction&) const;
    bool maybe_load(Blob&);

    static
    bool blob_is_dir(Blob&);

    friend std::ostream& operator<<(std::ostream&, const Directory&);

private:
    NameMap _name_map;
};

} // namespace
