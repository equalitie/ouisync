#include "branch_view.h"
#include "error.h"
#include "refcount.h"
#include "object_store.h"
#include "object/tagged.h"
#include "error.h"
#include "multi_dir.h"

#include <boost/filesystem.hpp>
#include <boost/serialization/vector.hpp>
#include <boost/serialization/set.hpp>
#include <iostream>

using namespace ouisync;
using object::Tree;
using object::Blob;
using std::set;
using std::map;
using std::string;

//--------------------------------------------------------------------

static
PathRange _parent(PathRange path) {
    path.advance_end(-1);
    return path;
}

//--------------------------------------------------------------------
MultiDir BranchView::root() const
{
    return MultiDir{{_root_id}, &_objects};
}
//--------------------------------------------------------------------

BranchView::BranchView(ObjectStore& objects, const ObjectId& root_id) :
    _objects(objects),
    _root_id(root_id)
{}

//--------------------------------------------------------------------

set<string> BranchView::readdir(PathRange path) const
{

    MultiDir dir = root().cd_into(path);

    set<string> names;

    for (auto& [name, object_id] : dir.list()) {
        names.insert(name);
    }

    return names;
}

//--------------------------------------------------------------------

FileSystemAttrib BranchView::get_attr(PathRange path) const
{
    if (path.empty()) return FileSystemDirAttrib{};

    MultiDir dir = root().cd_into(_parent(path));

    auto file_id = dir.file(path.back());

    auto obj = _objects.load<Tree::Nothing, Blob::Size>(file_id);

    FileSystemAttrib attrib;

    apply(obj,
        [&] (const Tree::Nothing&) { attrib = FileSystemDirAttrib{}; },
        [&] (const Blob::Size& b) { attrib = FileSystemFileAttrib{b.value}; });

    return attrib;
}

//--------------------------------------------------------------------

size_t BranchView::read(PathRange path, const char* buf, size_t size, size_t offset) const
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    MultiDir dir = root().cd_into(_parent(path));

    auto blob = _objects.load<Blob>(dir.file(path.back()));

    size_t len = blob.size();

    if (size_t(offset) < len) {
        if (offset + size > len) size = len - offset;
        memcpy((void*)buf, blob.data() + offset, size);
    } else {
        size = 0;
    }

    return size;
}

//--------------------------------------------------------------------

static
void _show(std::ostream& os, ObjectStore& objects, ObjectId id, std::string pad = "") {
    if (!objects.exists(id)) {
        os << pad << "!!! object " << id << " does not exist !!!\n";
        return;
    }

    auto obj = objects.load<Tree, Blob>(id);
    auto rc = objects.rc(id);

    apply(obj,
            [&] (const Tree& t) {
                os << pad << "Tree ID:" << t.calculate_id() << " (" << rc << ")\n";
                for (auto& [name, name_map] : t) {
                    for (auto& [user, vobj] : name_map) {
                        os << pad << "  U: " << user << "\n";
                        _show(os, objects, vobj.object_id, pad + "    ");
                    }
                }
            },
            [&] (const Blob& b) {
                os << pad << b << " (" << rc << ")\n";
            });
}

void BranchView::show(std::ostream& os) const
{
    return _show(os, _objects, _root_id, "");
}

//--------------------------------------------------------------------

bool BranchView::object_exists(const ObjectId& id) const
{
    return _objects.exists(id);
}

//--------------------------------------------------------------------
