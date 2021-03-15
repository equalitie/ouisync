#include "branch.h"
#include "error.h"
#include "path_range.h"
#include "archive.h"
#include "ouisync_assert.h"
#include "directory.h"
#include "multi_dir.h"

#include "branch/root_op.h"
#include "branch/cd_op.h"
#include "branch/file_op.h"
#include "branch/remove_op.h"

#include <boost/filesystem.hpp>
#include <boost/serialization/set.hpp>

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
Branch Branch::create(executor_type ex, const fs::path& path, UserId user_id, BlockStore& block_store, Options::Branch options)
{
    if (fs::exists(path)) {
        throw std::runtime_error("Local branch already exits");
    }

    Branch b(ex, path, user_id, block_store, move(options));

    const Directory empty_dir;
    Transaction tnx;
    auto empty_dir_id = empty_dir.save(tnx);

    b._index = Index(user_id, {empty_dir_id, {}});

    tnx.commit(user_id, block_store, b._index);

    b.store_self();

    return b;
}

/* static */
Branch Branch::load(executor_type ex, const fs::path& file_path, UserId user_id, BlockStore& block_store, Options::Branch options)
{
    Branch b(ex, file_path, user_id, block_store, move(options));
    archive::load(file_path, b);
    return b;
}

//--------------------------------------------------------------------

Branch::Branch(executor_type ex, const fs::path& file_path, const UserId& user_id,
        BlockStore& block_store, Options::Branch options) :
    _ex(ex),
    _file_path(file_path),
    _options(move(options)),
    _block_store(block_store),
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
    return make_unique<Branch::RootOp>(_block_store, _user_id, _index);
}

MultiDir Branch::root_multi_dir() const
{
    return MultiDir(_index.commits(), _block_store);
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

    auto blob = Blob::open(file_id, _block_store);

    if (Directory::blob_is_dir(blob)) {
        return FileSystemDirAttrib{};
    }

    auto file = File::open(blob);

    return FileSystemFileAttrib{file.size()};
}

//--------------------------------------------------------------------

size_t Branch::read(PathRange path, char* buf, size_t size, size_t offset) const
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    MultiDir dir = root_multi_dir().cd_into(parent(path));

    auto blob = Blob::open(dir.file(path.back()), _block_store);

    File file = File::open(blob);
    
    return file.read(buf, size, offset);
}

//--------------------------------------------------------------------

void Branch::mknod(PathRange path)
{
    // man 2 mknod

    if (path.empty()) {
        throw_error(sys::errc::file_exists);
    }

    auto file_op = get_file(path);

    if (file_op->file()) {
        throw_error(sys::errc::file_exists);
    }

    file_op->file() = File();
    do_commit(file_op);
}

//--------------------------------------------------------------------

size_t Branch::write(PathRange path, const char* buf, size_t size, size_t offset)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    auto file_op = get_file(path);

    if (!file_op->file()) {
        file_op->file() = File();
    }

    auto& file = *file_op->file();

    file.write(buf, size, offset);

    do_commit(file_op);

    return size;
}

//--------------------------------------------------------------------

size_t Branch::truncate(PathRange path, size_t size)
{
    if (path.empty()) throw_error(sys::errc::is_a_directory);

    auto file_op = get_file(path);

    if (!file_op->file()) {
        file_op->file() = File();
    }

    auto& file = *file_op->file();

    auto s = file.truncate(size);

    do_commit(file_op);

    return s;
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

    _index.merge(index, _block_store);
    store_self();

    if (debug) {
        cerr << "Result:\n" << _index << "\n";
        cerr << "----------------------------------------\n";
    }
}

//--------------------------------------------------------------------

void Branch::store(const Block& b)
{
    auto id = BlockStore::calculate_block_id(b);

    if (_index.mark_not_missing(id)) {
        _block_store.store(b);
        store_self();
    }
}

//--------------------------------------------------------------------

void Branch::store_self() const {
    archive::store(_file_path, *this);
}

//--------------------------------------------------------------------

static void print(std::ostream& os, const BlockId& block_id, BlockStore& block_store, unsigned level)
{
    auto pad = Padding(level*2);
    auto opt = Blob::maybe_open(block_id, block_store);

    if (!opt) {
        os << pad << "!!! Block " << block_id << " is not in BlockStore !!!\n";
        return;
    }

    Directory d;

    if (d.maybe_load(*opt)) {
        os << pad << "Directory id:" << block_id << "\n";
        for (auto& [filename, user_map] : d) {
            os << pad << "  " << filename << "/\n";
            for (auto& [user, vobj]: user_map) {
                os << pad << "    User:" << user << "\n";
                os << pad << "    Versions:" << vobj.versions << "\n";
                print(os, vobj.id, block_store, level + 2);
            }
        }
    } else if (auto f = File::maybe_open(*opt)) {
        os << pad << "File id:" << block_id << " size:" << f->size() << "\n";
    } else {
        os << pad << "!!! Block " << block_id << " is neither a File nor a Directory !!!\n";
    }
}

std::ostream& ouisync::operator<<(std::ostream& os, const Branch& branch)
{
    for (auto& [user, commit] : branch._index.commits()) {
        os << "User: " << user << " Commit: " << commit.id << " " << commit.versions << "\n";
        print(os, commit.id, branch._block_store, 1);
    }

    return os;
}
