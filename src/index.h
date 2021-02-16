#pragma once

#include "commit.h"

#include <map>
#include <set>
#include <string>

namespace ouisync {

class Index {
private:
    using Elements = std::map<ObjectId, std::map<ObjectId, uint32_t>>;
    //                                              |         |
    //                                   Parent <---+         |
    //                                                        |
    //  How many times is the object listed in the parent <---+

public:
    Index();

    void set_version_vector(const VersionVector&);

    void insert_object(const ObjectId& id, const ObjectId& parent_id, size_t cnt = 1);
    void remove_object(const ObjectId& id, const ObjectId& parent_id);

    bool has(const ObjectId& obj_id) const;
    size_t count_object_in_parent(const ObjectId& obj_id, const ObjectId& parent_id) const;

    template<class Archive>
    void serialize(Archive& ar, unsigned) {
        ar & _commit & _elements;
    }

    const Commit& commit() const { return _commit; }

    template<class F> void remove_count(F&& f) {
        for (auto i = _elements.begin(); i != _elements.end();) {
            auto i_next = std::next(i);
            auto& parents = i->second;

            for (auto j = parents.begin(); j != parents.end(); ) {
                auto j_next = std::next(j);
                size_t cnt = f(i->first, j->first, j->second);
                ouisync_assert(cnt <= j->second);
                j->second -= cnt;
                if (j->second == 0) { parents.erase(j); }
                j = j_next;
            }

            if (parents.empty()) _elements.erase(i);

            i = i_next;
        }
    }

    template<class F> void for_each(F&& f) const {
        for (auto i = _elements.begin(); i != _elements.end(); ++i) {
            auto& parents = i->second;

            for (auto j = parents.begin(); j != parents.end(); ++j) {
                f(i->first, j->first, j->second);
            }
        }
    }

    friend std::ostream& operator<<(std::ostream&, const Index&);

private:
    Commit _commit;
    Elements _elements;
};

} // namespace
