#include "branch.h"
#include "variant.h"
#include "error.h"
#include "path_range.h"
#include "archive.h"
#include "ouisync_assert.h"
#include "multi_dir.h"

#include "branch/root_op.h"
#include "branch/cd_op.h"
#include "branch/file_op.h"
#include "branch/remove_op.h"

#include <boost/filesystem.hpp>
#include <boost/serialization/vector.hpp>
#include <boost/serialization/set.hpp>
#include <string.h> // memcpy

#include <iostream>
#include "ostream/padding.h"

using namespace ouisync;
using std::move;
using std::unique_ptr;
using std::make_unique;
using std::string;
using std::set;
using std::cerr;

#define DBG std::cerr << __PRETTY_FUNCTION__ << ":" << __LINE__ << " "

/* static */
Branch Branch::create(executor_type ex, const fs::path& path, UserId user_id, ObjectStore& objstore, Options::Branch options)
{
    if (fs::exists(path)) {
        throw std::runtime_error("Local branch already exits");
    }

    Branch b(ex, path, user_id, objstore, move(options));

    auto empty_dir_id = objstore.store(Directory{});

    b._index = Index(user_id, {empty_dir_id, {}});

    b.store_self();

    return b;
}

/* static */
Branch Branch::load(executor_type ex, const fs::path& file_path, UserId user_id, ObjectStore& objstore, Options::Branch options)
{
    Branch b(ex, file_path, user_id, objstore, move(options));
    archive::load(file_path, b);
    return b;
}

//--------------------------------------------------------------------

Branch::Branch(executor_type ex, const fs::path& file_path, const UserId& user_id,
        ObjectStore& objstore, Options::Branch options) :
    _ex(ex),
    _file_path(file_path),
    _options(move(options)),
    _objstore(objstore),
    _block_store(_objstore),
    _user_id(user_id),
    _state_change_wait(_ex)
{
}

//--------------------------------------------------------------------

static
PathRange parent(PathRange path) {
    path.advance_end(-1);
    return path;
}

//--------------------------------------------------------------------
template<class OpPtr>
void Branch::do_commit(OpPtr& op)
{
    if (op->commit()) {
        store_self();
        _state_change_wait.notify();
    }
}

//--------------------------------------------------------------------
unique_ptr<Branch::DirectoryOp> Branch::root_op()
{
    return make_unique<Branch::RootOp>(_objstore, _user_id, _index);
}

MultiDir Branch::root_multi_dir() const
{
    return MultiDir(_index.commits(), _objstore);
}

unique_ptr<Branch::DirectoryOp> Branch::cd_into(PathRange path)
{
    unique_ptr<DirectoryOp> dir = root_op();

    for (auto& p : path) {
        dir = make_unique<Branch::CdOp>(move(dir), _user_id, p);
    }

    return dir;
}

unique_ptr<Branch::FileOp> Branch::get_file(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    unique_ptr<DirectoryOp> dir = cd_into(parent(path));

    return make_unique<FileOp>(move(dir), _user_id, path.back());
}

//--------------------------------------------------------------------

set<string> Branch::readdir(PathRange path) const
{
    MultiDir dir = root_multi_dir().cd_into(path);
    return dir.list();
}

//--------------------------------------------------------------------

FileSystemAttrib Branch::get_attr(PathRange path) const
{
    if (path.empty()) return FileSystemDirAttrib{};

    MultiDir dir = root_multi_dir().cd_into(parent(path));

    auto file_id = dir.file(path.back());

    auto obj = _objstore.load<Directory::Nothing, FileBlob::Size>(file_id);

    FileSystemAttrib attrib;

    apply(obj,
        [&] (const Directory::Nothing&) { attrib = FileSystemDirAttrib{}; },
        [&] (const FileBlob::Size& b) { attrib = FileSystemFileAttrib{b.value}; });

    return attrib;
}

//--------------------------------------------------------------------

size_t Branch::read(PathRange path, const char* buf, size_t size, size_t offset) const
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    MultiDir dir = root_multi_dir().cd_into(parent(path));

    auto blob = _objstore.load<FileBlob>(dir.file(path.back()));

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

void Branch::mknod(PathRange path)
{
    // man 2 mknod

    if (path.empty()) {
        throw_error(sys::errc::file_exists);
    }

    auto file = get_file(path);

    if (file->blob()) {
        throw_error(sys::errc::file_exists);
    }

    file->blob() = FileBlob{};
    do_commit(file);
}

//--------------------------------------------------------------------

size_t Branch::write(PathRange path, const char* buf, size_t size, size_t offset)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    auto file = get_file(path);

    if (!file->blob()) {
        file->blob() = FileBlob{};
    }

    auto& blob = *file->blob();

    size_t len = blob.size();

    if (offset + size > len) {
        blob.resize(offset + size);
    }

    memcpy(blob.data() + offset, buf, size);

    do_commit(file);

    return size;
}

//--------------------------------------------------------------------

size_t Branch::truncate(PathRange path, size_t size)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    auto file = get_file(path);

    if (!file->blob()) {
        file->blob() = FileBlob{};
    }

    auto& blob = *file->blob();

    blob.resize(std::min<size_t>(blob.size(), size));

    do_commit(file);

    return blob.size();
}

//--------------------------------------------------------------------

void Branch::mkdir(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::invalid_argument);

    unique_ptr<DirectoryOp> dir = cd_into(parent(path));

    if (dir->tree().find(path.back())) throw_error(sys::errc::file_exists);

    dir = make_unique<Branch::CdOp>(move(dir), _user_id, path.back(), true);

    do_commit(dir);
}

//--------------------------------------------------------------------

bool Branch::remove(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::operation_not_permitted);

    auto dir = cd_into(parent(path));
    auto rm = make_unique<Branch::RemoveOp>(move(dir), path.back());

    do_commit(rm);
    return true;
}

bool Branch::remove(const fs::path& fspath)
{
    Path path(fspath);
    return remove(path);
}

//--------------------------------------------------------------------

void Branch::merge_index(const Index& index)
{
    static constexpr bool debug = false;

    if (debug) {
        cerr << "----------------------------------------\n";
        cerr << "Branch::merge_index\n";
        cerr << "Old:\n" << _index << "\n";
        cerr << "Merge with:\n" << index << "\n";
        cerr << "------------------\n";
    }

    _index.merge(index, _objstore);

    if (debug) {
        cerr << "Result:\n" << _index << "\n";
        cerr << "----------------------------------------\n";
    }
}

//--------------------------------------------------------------------

void Branch::store(const FileBlob& f)
{
    auto id = f.calculate_id();

    if (_index.mark_not_missing(id)) {
        _objstore.store(f);
    }
}

void Branch::store(const Directory& d)
{
    auto id = d.calculate_id();

    if (_index.mark_not_missing(id)) {
        _objstore.store(d);
    }
}

//--------------------------------------------------------------------

void Branch::store_self() const {
    archive::store(_file_path, *this);
}

//--------------------------------------------------------------------

static void print(std::ostream& os, const ObjectId& obj_id, ObjectStore& objstore, unsigned level)
{
    auto pad = Padding(level*2);
    auto opt = objstore.maybe_load<Directory, FileBlob::Size>(obj_id);

    if (!opt) {
        os << pad << "!!! Object " << obj_id << " is not in ObjectStore !!!\n";
        return;
    }

    apply(*opt,
            [&] (const Directory& d) {
                os << pad << "Directory id:" << obj_id << "\n";
                for (auto& [filename, user_map] : d) {
                    os << pad << "  " << filename << "/\n";
                    for (auto& [user, vobj]: user_map) {
                        os << pad << "    User:" << user << "\n";
                        os << pad << "    Versions:" << vobj.versions << "\n";
                        print(os, vobj.id, objstore, level + 2);
                    }
                }
            },
            [&] (const FileBlob::Size& f) {
                os << pad << "File id:" << obj_id << " size:" << f.value << "\n";
            });

}

std::ostream& ouisync::operator<<(std::ostream& os, const Branch& branch)
{
    for (auto& [user, commit] : branch._index.commits()) {
        os << "User: " << user << " Commit: " << commit.id << " " << commit.versions << "\n";
        print(os, commit.id, branch._objstore, 1);
    }

    return os;
}
