#include "branch_view.h"
#include "error.h"
#include "object_store.h"
#include "error.h"
#include "multi_dir.h"

#include <boost/filesystem.hpp>
#include <boost/serialization/vector.hpp>
#include <boost/serialization/set.hpp>
#include <iostream>

using namespace ouisync;
using std::set;
using std::map;
using std::string;
using std::move;

//--------------------------------------------------------------------

static
PathRange _parent(PathRange path) {
    path.advance_end(-1);
    return path;
}

//--------------------------------------------------------------------
MultiDir BranchView::root() const
{
    return MultiDir(_commits, _objects);
}
//--------------------------------------------------------------------

BranchView::BranchView(ObjectStore& objects, map<UserId, VersionedObject> commits) :
    _objects(objects),
    _commits(move(commits))
{}

//--------------------------------------------------------------------

static
void _show(std::ostream& os, ObjectStore& objects, ObjectId id, std::string pad = "") {
    if (!objects.exists(id)) {
        os << pad << "!!! object " << id << " does not exist !!!\n";
        return;
    }

    auto obj = objects.load<Directory, FileBlob>(id);

    apply(obj,
            [&] (const Directory& d) {
                os << pad << "Directory ID:" << d.calculate_id() << "\n";
                for (auto& [name, name_map] : d) {
                    for (auto& [user, vobj] : name_map) {
                        os << pad << "  U: " << user << "\n";
                        _show(os, objects, vobj.id, pad + "    ");
                    }
                }
            },
            [&] (const FileBlob& b) {
                os << pad << b << "\n";
            });
}

void BranchView::show(std::ostream& os) const
{
    //return _show(os, _objects, _root_id, "");
}

//--------------------------------------------------------------------

bool BranchView::object_exists(const ObjectId& id) const
{
    return _objects.exists(id);
}

//--------------------------------------------------------------------
