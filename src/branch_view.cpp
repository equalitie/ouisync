#include "branch_view.h"
#include "error.h"
#include "refcount.h"
#include "object_store.h"
#include "object/tagged.h"
#include "error.h"

#include <boost/filesystem.hpp>
#include <boost/serialization/vector.hpp>

using namespace ouisync;
using object::Tree;
using object::Blob;
using std::set;
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

struct MultiDir {
    set<ObjectId> ids;
    ObjectStore* objstore;

    MultiDir cd_into(const std::string& where)
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
};

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

//class ConflictNameAssigner {
//public:
//    ConflictNameAssigner(string name_root) : _root(std::move(name_root)) {}
//
//    void add(const Tree::VersionedIds& versioned_ids)
//    {
//    }
//
//private:
//    string _root;
//};

//--------------------------------------------------------------------

BranchView::BranchView(ObjectStore& objects, const ObjectId& root_id) :
    _objects(objects),
    _root_id(root_id)
{}

//--------------------------------------------------------------------

set<string> BranchView::readdir(PathRange path) const
{
    set<string> files;

    MultiDir dir{{_root_id}, &_objects};

    for (auto& p : path) {
        dir = dir.cd_into(p);
    }

    for (auto& id : dir.ids) {
        auto tree = _objects.load<Tree>(id);
        for (auto& name : tree.children_names()) {
            files.insert(name);
        }
    }

    return files;
}

//--------------------------------------------------------------------

FileSystemAttrib BranchView::get_attr(PathRange path) const
{
    throw std::runtime_error("not implemented yet");
    return {};

    //if (path.empty()) return FileSystemDirAttrib{};

    //FileSystemAttrib attrib;

    //_query_dir(_objects, _root_id, _parent(path),
    //    [&] (const Tree& parent) {
    //        auto child = parent.find(path.back());
    //        if (!child) throw_error(sys::errc::no_such_file_or_directory);

    //        auto obj = _objects.load<Tree::Nothing, Blob::Size>(child.id());

    //        apply(obj,
    //            [&] (const Tree::Nothing&) { attrib = FileSystemDirAttrib{}; },
    //            [&] (const Blob::Size& b) { attrib = FileSystemFileAttrib{b.value}; });
    //    });

    //return attrib;
}

//--------------------------------------------------------------------

size_t BranchView::read(PathRange path, const char* buf, size_t size, size_t offset) const
{
    throw std::runtime_error("not implemented yet");
    return 0;

    //if (path.empty()) throw_error(sys::errc::is_a_directory);

    //_query_dir(_objects, _root_id, _parent(path),
    //    [&] (const Tree& tree) {
    //        auto child = tree.find(path.back());
    //        if (!child) throw_error(sys::errc::no_such_file_or_directory);

    //        // XXX: Read only what's needed, not the whole blob
    //        auto blob = _objects.load<Blob>(child.id());

    //        size_t len = blob.size();

    //        if (size_t(offset) < len) {
    //            if (offset + size > len) size = len - offset;
    //            memcpy((void*)buf, blob.data() + offset, size);
    //        } else {
    //            size = 0;
    //        }
    //    });

    //return size;
}

//--------------------------------------------------------------------

Opt<Blob> BranchView::maybe_load(PathRange path) const
{
    throw std::runtime_error("not implemented yet");
    return  boost::none;

    //if (path.empty()) throw_error(sys::errc::is_a_directory);

    //Opt<Blob> retval;

    //_query_dir(_objects, _root_id, _parent(path),
    //    [&] (const Tree& tree) {
    //        auto child = tree.find(path.back());
    //        if (!child) throw_error(sys::errc::no_such_file_or_directory);
    //        retval = _objects.load<Blob>(child.id());
    //    });

    //return retval;
}

//--------------------------------------------------------------------

ObjectId BranchView::id_of(PathRange path) const
{
    throw std::runtime_error("not implemented yet");
    return {};

    //if (path.empty()) return _root_id;

    //ObjectId retval;

    //_query_dir(_objects, _root_id, _parent(path),
    //    [&] (const Tree& tree) {
    //        auto child = tree.find(path.back());
    //        if (!child) throw_error(sys::errc::no_such_file_or_directory);
    //        retval = child.id();
    //    });

    //return retval;
}

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
