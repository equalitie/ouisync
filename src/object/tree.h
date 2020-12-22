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

class Tree final : private std::map<std::string, ObjectId> {
private:
    using Parent = std::map<std::string, ObjectId>;

    struct Erase { ObjectId id; };
    struct Move { ObjectId from, to; };
    struct Insert { ObjectId id; };

    using Edit = variant<Erase, Move, Insert>;

    template<bool is_mutable>
    class Handle {
        using TreePtr = std::conditional_t<is_mutable, Tree*, const Tree*>;
        using Iterator = std::conditional_t<is_mutable, Parent::iterator, Parent::const_iterator>;

      public:
        Handle() = default;
        Handle(const Handle&) = default;
        Handle& operator=(const Handle&) = default;

        Handle(Handle&& other) : tree(other.tree), i(other.i)
        {
            other.tree = nullptr;
            other.i = Parent::iterator();
        }

        Handle& operator=(Handle&& other)
        {
            tree = other.tree;
            i = other.i;
            other.tree = nullptr;
            other.i = Parent::iterator();
            return *this;
        }

        ObjectId id() const {
            assert(tree);
            if (!tree) exit(1);
            return i->second;
        }

        operator bool() const { return tree != nullptr; }

        void set_id(const ObjectId& id) {
            assert(tree);
            if (!tree) exit(1);
            if (id == i->second) return;
            i->second = id;
        }

      private:
        friend class Tree;
        Handle(TreePtr tree, Iterator i)
            : tree(tree), i(i) {}

      private:
        TreePtr tree = nullptr;
        Iterator i;
    };

public:
    auto begin() const { return Parent::begin(); }
    auto end()   const { return Parent::end();   }

    using Parent::size;

    using MutableHandle   = Handle<true>;
    using ImmutableHandle = Handle<false>;

    ImmutableHandle find(const std::string& k) const {
        auto i = Parent::find(k);
        if (i == Parent::end()) return {};
        return {this, i};
    }

    MutableHandle find(const std::string& k) {
        auto i = Parent::find(k);
        if (i == Parent::end()) return {};
        return {this, i};
    }

    void erase(const MutableHandle& h)
    {
        assert(h.tree);
        assert(h.tree == this);
        if (h.tree != this) exit(1);
        Parent::erase(h.i);
    }

    std::pair<MutableHandle, bool>
    insert(std::pair<std::string, ObjectId> pair) {
        auto [i,inserted] = Parent::insert(std::move(pair));
        return {MutableHandle{this, i}, inserted};
    }

    MutableHandle operator[](std::string key) {
        return insert({std::move(key), ObjectId{}}).first;
    }

    std::set<ObjectId> children() const {
        std::set<ObjectId> ch;
        for (auto& [_, id] : *this) { ch.insert(id); }
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
        ar & static_cast<Parent&>(*this);
    }

    ObjectId calculate_id() const;

    friend std::ostream& operator<<(std::ostream&, const Tree&);
};



} // namespace

