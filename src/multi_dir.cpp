#include "multi_dir.h"
#include "directory.h"
#include "object_store.h"
#include "variant.h"
#include "error.h"

#include <sstream>

using namespace ouisync;
using std::map;
using std::string;
using std::stringstream;
using std::move;

class ConflictNameAssigner {
public:
    using Name = string;

    ConflictNameAssigner(string name_root) :
        _name_root(move(name_root)) {}

    void add(const Directory::UserMap& usr_map)
    {
        for (auto& [user_id, vobj] : usr_map) {
            _versions[user_id] = vobj.object_id;
        }
    }

    map<Name, ObjectId> resolve() const
    {
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
            stringstream ss;
            ss << _name_root << "-" << (cnt++);
            ret[ss.str()] = object_id;
        }

        return ret;
    }

private:
    string _name_root;
    map<UserId, ObjectId> _versions;
};

MultiDir MultiDir::cd_into(const string& where) const
{
    MultiDir retval{{}, objstore};

    for (auto& from_id : ids) {
        const auto obj = objstore->load<Directory, FileBlob::Nothing>(from_id);
        auto tree = boost::get<Directory>(&obj);
        if (!tree) continue;
        auto user_map = tree->find(where);
        for (auto& [user_id, vobj] : user_map) {
            retval.ids.insert(vobj.object_id);
        }
    }

    return retval;
}

MultiDir MultiDir::cd_into(PathRange path) const
{
    MultiDir result = *this;
    for (auto& p : path) { result = result.cd_into(p); }
    return result;
}

ObjectId MultiDir::file(const string& name) const
{
    // XXX: using `list()` is an overkill here.
    auto lst = list();
    auto i = lst.find(name);
    if (i == lst.end()) throw_error(sys::errc::no_such_file_or_directory);
    return i->second;
}

map<string, ObjectId> MultiDir::list() const {
    map<string, ConflictNameAssigner> name_resolvers;

    for (auto& id : ids) {
        auto tree = objstore->load<Directory>(id);
        for (auto& [name, versioned_ids] : tree) {
            auto [i, _] = name_resolvers.insert({name, {name}});
            i->second.add(versioned_ids);
        }
    }

    map<string, ObjectId> result;

    for (auto& [_, name_resolver] : name_resolvers) {
        for (auto& [name, object_id] : name_resolver.resolve()) {
            result.insert({name, object_id});
        }
    }

    return result;
}

