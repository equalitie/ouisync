#include "multi_dir.h"
#include "directory.h"
#include "object_store.h"
#include "variant.h"
#include "error.h"

#include <sstream>
#include <iostream>

using namespace ouisync;
using std::set;
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
            _versions[user_id] = vobj.id;
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

Opt<MultiDir::Version> MultiDir::pick_subdirectory_to_edit(
        const UserId& preferred_user, const string_view name)
{
    auto i = versions.find(preferred_user);

    if (i != versions.end()) {
        return Version{preferred_user, i->second};
    }

    Opt<Version> ret;

    for (auto& [subdir_user, subdir_vobj] : versions) {
        if (!ret) {
            ret = Version{subdir_user, subdir_vobj};
            continue;
        }

        if (ret->user == preferred_user) {
            if (subdir_user != preferred_user) continue;
            if (ret->vobj.has_smaller_user_version(subdir_vobj, preferred_user)) {
                ret->vobj = subdir_vobj;
            }
        }
        else {
            if (subdir_user == preferred_user) {
                ret->user = subdir_user;
                ret->vobj = subdir_vobj;
            }
            else {
                if (ret->vobj.happened_before(subdir_vobj)) {
                    ret->user = subdir_user;
                    ret->vobj = subdir_vobj;
                }
                else {
                    // XXX: Use some kind of heuristic to try to get the
                    // most recent vobj out of all concurrent ones.
                }
            }
        }
    }

    return ret;
}

bool MultiDir::has_subdirectory(string_view name) const
{
    for (auto& [user, vobj] : versions) {
        // XXX: Should be cached
        const auto od = objstore->maybe_load<Directory>(vobj.id);
        if (!od) continue;
        auto user_map = od->find(name);
        if (!user_map) continue;

        for (auto& [user_id, vobj] : user_map) {
            // XXX: Would be faster to have this information in vobj.
            auto is_dir = objstore->maybe_load<Directory::Nothing>(vobj.id);
            if (is_dir) return true;
        }
    }
    return false;
}

MultiDir MultiDir::cd_into(const string& where) const
{
    MultiDir retval({}, *objstore);

    for (auto& [user, vobj] : versions) {
        const auto obj = objstore->load<Directory, FileBlob::Nothing>(vobj.id);
        auto tree = boost::get<Directory>(&obj);
        if (!tree) continue;
        auto user_map = tree->find(where);
        for (auto& [user_id, vobj] : user_map) {
            retval.versions.insert({user_id, vobj});
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
    for (auto& [user, vobj] : versions) {
        auto dir = objstore->load<Directory>(vobj.id);

        auto usermap = dir.find(name);
        if (!usermap) continue;

        // XXX: resolve conflicts
        return usermap.begin()->second.id;
    }

    throw_error(sys::errc::no_such_file_or_directory);
    return {};
}

set<string> MultiDir::list() const {
    set<string> ret;

    for (auto& [user, vobj] : versions) {
        auto dir = objstore->load<Directory>(vobj.id);
        for (auto& [filename, _] : dir) {
            ret.insert(filename);
        }
    }

    return ret;
}

