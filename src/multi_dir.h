#pragma once

#include "object/blob.h"
#include "conflict_name_assigner.h"

namespace ouisync {

class MultiDir {
public:
    using Blob = object::Blob;

    std::set<ObjectId> ids;
    ObjectStore* objstore;

    MultiDir cd_into(const std::string& where) const
    {
        MultiDir retval{{}, objstore};
    
        for (auto& from_id : ids) {
            const auto obj = objstore->load<Directory, Blob::Nothing>(from_id);
            auto tree = boost::get<Directory>(&obj);
            if (!tree) continue;
            auto user_map = tree->find(where);
            for (auto& [user_id, vobj] : user_map) {
                retval.ids.insert(vobj.object_id);
            }
        }
    
        return retval;
    }

    MultiDir cd_into(PathRange path) const
    {
        MultiDir result = *this;
        for (auto& p : path) { result = result.cd_into(p); }
        return result;
    }

    std::map<std::string, ObjectId> list() const {
        std::map<std::string, ConflictNameAssigner> name_resolvers;

        for (auto& id : ids) {
            auto tree = objstore->load<Directory>(id);
            for (auto& [name, versioned_ids] : tree) {
                auto [i, _] = name_resolvers.insert({name, {name}});
                i->second.add(versioned_ids);
            }
        }

        std::map<std::string, ObjectId> result;

        for (auto& [_, name_resolver] : name_resolvers) {
            for (auto& [name, object_id] : name_resolver.resolve()) {
                result.insert({name, object_id});
            }
        }

        return result;
    }

    ObjectId file(const std::string& name) const
    {
        // XXX: using `list()` is an overkill here.
        auto lst = list();
        auto i = lst.find(name);
        if (i == lst.end()) throw_error(sys::errc::no_such_file_or_directory);
        return i->second;
    }
};



}
