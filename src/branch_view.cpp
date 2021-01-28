#include "branch_view.h"
#include "error.h"
#include "refcount.h"
#include "object_store.h"
#include "object/tagged.h"
#include "error.h"

#include <boost/filesystem.hpp>
#include <boost/serialization/vector.hpp>
#include <boost/serialization/set.hpp>

using namespace ouisync;
using object::Tree;
using object::Blob;
using std::set;
using std::map;
using std::string;

//--------------------------------------------------------------------

//template<class F>
//static
//void _query_dir(ObjectStore& objects, ObjectId tree_id, PathRange path, F&& f)
//{
//    const Tree tree = objects.load<Tree>(tree_id);
//
//    if (path.empty()) {
//        f(tree);
//    } else {
//        auto child = tree.find(path.front());
//        if (!child) throw_error(sys::errc::no_such_file_or_directory);
//        path.advance_begin(1);
//        _query_dir(objects, child.id(), path, std::forward<F>(f));
//    }
//}

static
PathRange _parent(PathRange path) {
    path.advance_end(-1);
    return path;
}

//map<ObjectId, set<VersionVector>> file(ObjectStore& objstore, const set<ObjectId>& dirs, const std::string& name)
//{
//    map<ObjectId, set<VersionVector>> retval;
//
//    for (auto& dir : dirs) {
//        const auto obj = objects.load<Tree, Blob::Nothing>(tree_id);
//        auto tree = boost::get<Tree>(&obj);
//        if (!tree) continue;
//        auto versions = tree.find(name);
//        for (auto& [id, clock] : versions) {
//            retval[id].;
//        }
//    }
//
//    return retval;
//}

//--------------------------------------------------------------------

class ConflictNameAssigner {
public:
    using Name = string;

    ConflictNameAssigner(string name_root) :
        _name_root(std::move(name_root)) {}

    void add(const Tree::VersionedIds& versioned_ids)
    {
        for (auto& [id, vvs] : versioned_ids) {
            for (auto& vv : vvs) {
                _versions[id].insert(vv);
            }
        }
    }

    map<Name, ObjectId> resolve() const
    {
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
    string _name_root;
    map<ObjectId, set<VersionVector>> _versions;
};

class ouisync::MultiDir {
public:
    set<ObjectId> ids;
    ObjectStore* objstore;

    MultiDir cd_into(const std::string& where) const
    {
        MultiDir retval{{}, objstore};
    
        for (auto& from_id : ids) {
            const auto obj = objstore->load<Tree, Blob::Nothing>(from_id);
            auto tree = boost::get<Tree>(&obj);
            if (!tree) continue;
            auto versions = tree->find(where);
            for (auto& [id, clock] : versions) {
                retval.ids.insert(id);
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

    map<string, ObjectId> list() const {
        map<string, ConflictNameAssigner> name_resolvers;

        for (auto& id : ids) {
            auto tree = objstore->load<Tree>(id);
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

    ObjectId file(const std::string& name) const
    {
        // XXX: using `list()` is an overkill here.
        auto lst = list();
        auto i = lst.find(name);
        if (i == lst.end()) throw_error(sys::errc::no_such_file_or_directory);
        return i->second;
    }
};

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

//ObjectId BranchView::id_of(PathRange path) const
//{
//    if (path.empty()) return _root_id;
//
//    // XXX: This won't work for directories
//    return root().cd_into(_parent(path)).file(path.back());
//}

//--------------------------------------------------------------------

static
void _show(std::ostream& os, ObjectStore& objects, ObjectId id, std::string pad = "") {
    //if (!objects.exists(id)) {
    //    os << pad << "!!! object " << id << " does not exist !!!\n";
    //    return;
    //}

    //auto obj = objects.load<Tree, Blob>(id);
    //auto rc = objects.rc(id);

    //apply(obj,
    //        [&] (const Tree& t) {
    //            os << pad << t << " (" << rc << ")\n";
    //            for (auto& [name, id] : t) {
    //                _show(os, objects, id, pad + "  ");
    //            }
    //        },
    //        [&] (const Blob& b) {
    //            os << pad << b << " (" << rc << ")\n";
    //        });
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
