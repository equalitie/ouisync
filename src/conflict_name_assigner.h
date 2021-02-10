#pragma once

#include "directory.h"

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

    void add(const Directory::UserMap& usr_map)
    {
        for (auto& [user_id, vobj] : usr_map) {
            _versions[user_id] = vobj.object_id;
        }
    }

    std::map<Name, ObjectId> resolve() const
    {
        using std::map;

        map<Name, ObjectId> ret;

        if (_versions.empty()) return ret;

        if (_versions.size() == 1) {
            ret.insert({_name_root, _versions.begin()->second});
            return ret;
        }

        // TODO: Need a proper way to indentify different versions. With this
        // simplistic version it could happen that the user opens a file with
        // one name, but it could change before it is saved.
        unsigned cnt = 0;

        for (auto& [user_id, object_id] : _versions) {
            std::stringstream ss;
            ss << _name_root << "-" << (cnt++);
            ret[ss.str()] = object_id;
        }

        return ret;
    }

private:
    std::string _name_root;
    std::map<UserId, ObjectId> _versions;
};

}
