#pragma once

namespace ouisync {

// Helper class to simplify iterating over collections while also being
// able to modify it. For example, to be able to iterate over elements
// in a map while also being able to delete some of the elements, one
// would need to do:
//
// for (auto i = map.begin(); i != map.end();) {
//   auto n = std::next(i);
//   if (foo(*i) map.erase(i);
//   i = n;
// }
//
// With this class, one can do (the improvement is more notable with nested
// loops):
//
// for (auto i : iterator_range(map)) {
//   if (foo(*i)) map.erase(i);
// }


template<class Collection>
struct IteratorRange {
    struct Iterator {
        using I = typename Collection::iterator;
        using iterator_category = typename I::iterator_category;
        using difference_type   = typename I::difference_type;
        using value_type        = I;
        using pointer           = I*;  // or also value_type*
        using reference         = I&;  // or also value_type&

              I& operator*() { return now; }
        const I& operator*() const { return now; }

        pointer operator->() { return *now; }

        // Prefix increment
        Iterator& operator++() {
            if (now != end) {
                now = next;
                if (next != end) next = std::next(next);
            }
            return *this;
        }  

        // Postfix increment
        Iterator operator++(int) { Iterator tmp = *this; ++(*this); return tmp; }

        friend bool operator== (const Iterator& a, const Iterator& b) { return a.now == b.now; };
        friend bool operator!= (const Iterator& a, const Iterator& b) { return a.now != b.now; };     

        I now;
        I next;
        I end;
    };

    Iterator begin() {
        auto b = collection.begin();
        auto e = collection.end();
        if (b == e) {
            return {b,b,e};
        } else {
            return {b, std::next(b), e};
        }
    }

    Iterator end() {
        auto e = collection.end();
        return {e,e,e};
    }

    Collection& collection;
};

template<class Collection>
IteratorRange<Collection> iterator_range(Collection& c) {
    return IteratorRange<Collection>{c};
}

} // namespace
