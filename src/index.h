#pragma once

#include "object_id.h"

#include <map>
#include <set>
#include <string>

namespace ouisync {

class ObjectStore;

class Index {
private:
    using Elements = std::map<ObjectId, std::set<Sha256::Digest>>;
    //                                          |______________|
    //                                                 |
    //           Hash(parent.object_id + file-name)  <-+

public:
    Index(ObjectStore&);

    void set_root(const ObjectId& id);

    void insert_object(const ObjectId& id, const std::string& filename, const ObjectId& parent_id);

    template<class Archive>
    void serialize(Archive& ar, unsigned) {
        ar & _elements;
    }

private:
    void remove_object(const ObjectId& id, const std::string& filename, const ObjectId& parent_id);

private:
    ObjectStore& _objstore;
    Opt<ObjectId> _root;
    Elements _elements;
};

} // namespace
