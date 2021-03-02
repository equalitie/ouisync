#include "multi_dir.h"
#include "directory.h"
#include "block_store.h"
#include "variant.h"
#include "error.h"
#include "file_blob.h"

#include <sstream>
#include <iostream>

using namespace ouisync;
using std::set;
using std::map;
using std::string;
using std::stringstream;
using std::move;

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

MultiDir MultiDir::cd_into(const string& where) const
{
    MultiDir retval({}, *block_store);

    for (auto& [user, vobj] : versions) {
        auto block = block_store->load(vobj.id);
        Directory tree;
        if (!tree.maybe_load(block)) continue;
        auto user_map = tree.find(where);
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
        auto block = block_store->load(vobj.id);
        Directory dir;
        if (!dir.maybe_load(block)) {
            throw std::runtime_error("MultiDir::file: Block is not a directory");
        }

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

    // XXX: Conflicting files - or directories that have been concurrently
    // modified and removed - need to marked as such.
    for (auto& [user, vobj] : versions) {
        auto block = block_store->load(vobj.id);
        Directory dir;
        if (!dir.maybe_load(block)) {
            throw std::runtime_error("MultiDir::list: Block is not a directory");
        }
        for (auto& [filename, _] : dir) {
            ret.insert(filename);
        }
    }

    return ret;
}

