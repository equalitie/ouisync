#pragma once

#include "object/tree.h"

#include <map>
#include <set>
#include <string>
#include <sstream>

namespace ouisync {

class ConflictNameAssigner {
public:
    using Name = std::string;

    ConflictNameAssigner(std::string name_root) :
        _name_root(std::move(name_root)) {}

    void add(const object::Tree::VersionedIds& versioned_ids)
    {
        for (auto& [id, meta] : versioned_ids) {
            _versions[id].insert(meta.version_vector);
        }
    }

    std::map<Name, ObjectId> resolve() const
    {
        using std::map;

        map<Name, ObjectId> ret;

        if (_versions.empty()) return ret;

        if (_versions.size() == 1) {
            ret.insert({_name_root, _versions.begin()->first});
            return ret;
        }

        // TODO: Need a proper way to indentify different versions. With this
        // simplistic version it could happen that the user opens a file with
        // one name, but it could change before it is saved.
        unsigned cnt = 0;

        for (auto& [obj_id, vv] : _versions) {
            std::stringstream ss;
            ss << _name_root << "-" << (cnt++);
            ret[ss.str()] = obj_id;
        }

        return ret;
    }

private:
    std::string _name_root;
    std::map<ObjectId, std::set<VersionVector>> _versions;
};

}
