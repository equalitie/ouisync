#pragma once

#include "commit.h"

#include <map>
#include <set>
#include <string>

namespace ouisync {

class ObjectStore;

class Index {
private:
    using Elements = std::map<ObjectId, std::map<ObjectId, uint32_t>>;
    //                                              |         |
    //                                   Parent <---+         |
    //                                                        |
    //  How many times is the object listed in the parent <---+

private:

    class Element {
      public:
        Element() {}
        Element(const Element&) = default;
        Element(Element&&) = default;

        Element(const ObjectId& id, const ObjectId& parent_id);
        Element(const ObjectId& id) : Element(id, id) {}

        bool is_root() const { return _is_root; }

        const ObjectId& obj_id() const { return _obj_id; }

      private:
        friend class Index;
        bool _is_root;
        ObjectId _obj_id;
        ObjectId _parent_id;
    };

public:
    void set_version_vector(const VersionVector&);

    void insert_object(const ObjectId& id, const ObjectId& parent_id);
    void remove_object(const ObjectId& id, const ObjectId& parent_id);

    bool has(const ObjectId& obj_id) const;
    bool has(const ObjectId& obj_id, const ObjectId& parent_id) const;

    template<class Archive>
    void serialize(Archive& ar, unsigned) {
        ar & _commit & _elements;
    }

    const Commit& commit() const { return _commit; }

private:
    Commit _commit;
    Elements _elements;
};

} // namespace
