#include "branch.h"
#include "object/tree.h"
#include "object/tagged.h"

#include <boost/filesystem.hpp>
#include <boost/archive/text_oarchive.hpp>
#include <boost/archive/text_iarchive.hpp>

using namespace ouisync;
using std::move;

/* static */
Branch Branch::load_or_create(const fs::path& rootdir, const fs::path& objdir, UserId user_id) {
    object::Id root_id;
    VersionVector clock;

    fs::path path = rootdir / user_id.to_string();

    fs::fstream file(path, file.binary | file.in);

    if (!file.is_open()) {
        object::Tree root_obj;
        root_id = root_obj.store(objdir);
        Branch branch{path, user_id, root_id, std::move(clock)};
        branch.store();
        return branch;
    }

    boost::archive::text_iarchive oa(file);
    object::tagged::Load<object::Id> load{root_id};
    oa >> load;
    oa >> clock;

    return Branch{path, user_id, root_id, move(clock)};
}

void Branch::store() {
    fs::fstream file(_file_path, file.binary | file.trunc | file.out);
    if (!file.is_open())
        throw std::runtime_error("Failed to open branch file");
    boost::archive::text_oarchive oa(file);
    object::tagged::Save<object::Id> save{_root_id};
    oa << save;
    oa << _clock;
}

void Branch::root_object_id(const object::Id& id) {
    auto old_id = _root_id;
    _root_id = id;
    if (_root_id != old_id) {
        _clock.increment(_user_id);
        store();
    }
}

Branch::Branch(const fs::path& file_path, const UserId& user_id, const object::Id& root_id, VersionVector clock) :
    _file_path(file_path), _user_id(user_id), _root_id(root_id), _clock(std::move(clock)) {}

