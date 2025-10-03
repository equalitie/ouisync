#pragma once

#include <ouisync/message.hpp>
#include <unordered_map>
#include <unordered_set>

namespace tests::print {
    template<class Collection> struct List {
        const Collection& collection;
    };
    template<class Collection>
    List<Collection> list(const Collection& c) {
        return List<Collection>{c};
    }
} // namespace tests::print

namespace std {
    ostream& operator<<(ostream& os, const ouisync::MessageId& m) {
        return os << "MessageId{" << m.value << "}\n"; 
    }
    template<class T>
    ostream& operator<<(ostream& os, const std::optional<T>& o) {
        if (o) return os << "Some{" << *o << "}";
        else return os << "None"; 
    }
    template<class Collection>
    ostream& operator<<(ostream& os, const tests::print::List<Collection>& l) {
        for (auto i = l.collection.begin(); i != l.collection.end(); ++i) {
            if (i != l.collection.begin()) os << ", ";
            os << *i;
        }
        return os;
    }
    template<class T1, class T2>
    ostream& operator<<(ostream& os, const std::pair<T1, T2>& p) {
        return os << "(" << p.first << ", " << p.second << ")";
    }
    template<class K, class V>
    ostream& operator<<(ostream& os, const std::unordered_map<K, V>& m) {
        return os << "Map{" << tests::print::list(m) << "}";
    }
    template<class V>
    ostream& operator<<(ostream& os, const std::unordered_set<V>& s) {
        return os << "Set{" << tests::print::list(s) << "}";
    }
} // namespace std

namespace ouisync {
    bool operator==(const MessageId& m1, const MessageId& m2) {
        return m1.value == m2.value;
    }
} // ouisync namespace

