#pragma once

#include "commit.h"

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
    Index(Commit, ObjectStore&);

    void set_root(const Commit&);

    void insert_object(const ObjectId& id, const std::string& filename, const ObjectId& parent_id);

    template<class Archive>
    void serialize(Archive& ar, unsigned) {
        ar & _commit & _elements;
    }

private:
    void remove_object(const ObjectId& id, const std::string& filename, const ObjectId& parent_id);

private:
    ObjectStore& _objstore;
    Commit _commit;
    Elements _elements;
};

} // namespace
