#include "file_system.h"
#include "snapshot.h"
#include "branch_io.h"

#include <iostream>
#include <boost/filesystem.hpp>
#include <boost/range/iterator_range.hpp>

using namespace ouisync;
using std::vector;
using std::map;
using std::string;
using boost::get;
using std::make_pair;
using Branch = FileSystem::Branch;

FileSystem::FileSystem(executor_type ex, Options options) :
    _ex(std::move(ex)),
    _options(std::move(options))
{
    _user_id = UserId::load_or_create(_options.user_id_file_path);

    for (auto f : fs::directory_iterator(_options.branchdir)) {
        auto branch = BranchIo::load(f, _options.objectdir);
        auto local_branch = boost::get<LocalBranch>(&branch);
        auto uid = local_branch->user_id();
        _branches.insert(make_pair(uid, std::move(*local_branch)));
    }

    if (_branches.count(_user_id) == 0) {
        _branches.insert(make_pair(_user_id,
                    LocalBranch::create(_options.branchdir, _options.objectdir, _user_id)));
    }
}

Branch& FileSystem::find_branch(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::invalid_argument);
    auto user_id = UserId::from_string(path.front().native());
    if (!user_id) throw_error(sys::errc::invalid_argument);
    auto i = _branches.find(*user_id);
    if (i == _branches.end()) throw_error(sys::errc::invalid_argument);
    return i->second;
}

static Commit _get_commit(const Branch& b) {
    return {
        apply(b, [] (const auto& b) { return b.version_vector(); }),
        apply(b, [] (const auto& b) { return b.root_object_id(); })
    };

}

Snapshot FileSystem::create_snapshot() const
{
    Snapshot::Commits commits;

    for (auto& [user_id, branch] : _branches) {
        (void) user_id;
        commits.insert(_get_commit(branch));
    }

    return Snapshot::create(_options.snapshotdir, _options.objectdir, std::move(commits));
}

net::awaitable<FileSystem::Attrib> FileSystem::get_attr(PathRange path)
{
    if (path.empty()) co_return DirAttrib{};

    auto& branch = find_branch(path);

    path.advance_begin(1);
    auto ret = apply(branch, [&] (auto& b) { return b.get_attr(path); });
    co_return ret;
}

net::awaitable<vector<string>> FileSystem::readdir(PathRange path)
{
    std::vector<std::string> nodes;

    if (path.empty()) {
        for (auto& [name, branch] : _branches) {
            (void) branch;
            nodes.push_back(name.to_string());
        }
    }
    else {
        auto& branch = find_branch(path);

        path.advance_begin(1);
        auto dir = apply(branch, [&](auto& b) { return b.readdir(path); });

        for (auto& [name, hash] : dir) {
            nodes.push_back(name);
        }
    }

    co_return nodes;
}

net::awaitable<size_t> FileSystem::read(PathRange path, char* buf, size_t size, off_t offset)
{
    if (path.empty()) {
        throw_error(sys::errc::invalid_argument);
    }

    auto& branch = find_branch(path);
    path.advance_begin(1);

    if (path.empty()) {
        throw_error(sys::errc::is_a_directory);
    }

    co_return apply(branch, [&] (auto& b) { return b.read(path, buf, size, offset); });
}

net::awaitable<size_t> FileSystem::write(PathRange path, const char* buf, size_t size, off_t offset)
{
    if (path.empty()) {
        throw_error(sys::errc::invalid_argument);
    }

    auto& branch = find_branch(path);
    path.advance_begin(1);

    if (path.empty()) {
        throw_error(sys::errc::is_a_directory);
    }

    co_return apply(branch,
            [&] (LocalBranch& b) -> size_t {
                return b.write(path, buf, size, offset);
            },
            [&] (RemoteBranch&) -> size_t {
                throw_error(sys::errc::operation_not_permitted);
                return 0; // Satisfy warning
            });
}

net::awaitable<void> FileSystem::mknod(PathRange path, mode_t mode, dev_t dev)
{
    if (S_ISFIFO(mode)) throw_error(sys::errc::invalid_argument); // TODO?

    if (path.empty()) {
        throw_error(sys::errc::invalid_argument);
    }

    auto& branch = find_branch(path);
    path.advance_begin(1);

    if (path.empty()) {
        throw_error(sys::errc::is_a_directory);
    }

    apply(branch,
        [&] (LocalBranch& b) {
            b.store(path, object::Blob{});
        },
        [&] (RemoteBranch&) {
            throw_error(sys::errc::operation_not_permitted);
        });

    co_return;
}

net::awaitable<void> FileSystem::mkdir(PathRange path, mode_t mode)
{
    if (path.empty()) {
        // The root directory is reserved for branches, users can't create
        // new directories there.
        throw_error(sys::errc::operation_not_permitted);
    }

    auto& branch = find_branch(path);
    path.advance_begin(1);

    apply(branch,
        [&] (LocalBranch& b) {
            b.mkdir(path);
        },
        [&] (RemoteBranch&) {
            throw_error(sys::errc::operation_not_permitted);
        });

    co_return;
}

net::awaitable<void> FileSystem::remove_file(PathRange path)
{
    if (path.empty()) {
        throw_error(sys::errc::is_a_directory);
    }

    auto& branch = find_branch(path);
    path.advance_begin(1);

    if (path.empty()) {
        // XXX: Branch removal not yet implemented
        throw_error(sys::errc::operation_not_permitted);
    }

    apply(branch,
        [&] (LocalBranch& b) {
            b.remove(path);
        },
        [&] (RemoteBranch&) {
            throw_error(sys::errc::operation_not_permitted);
        });

    co_return;
}

net::awaitable<void> FileSystem::remove_directory(PathRange path)
{
    if (path.empty()) {
        // XXX: Branch removal not yet implemented
        throw_error(sys::errc::operation_not_permitted);
    }

    auto& branch = find_branch(path);
    path.advance_begin(1);

    if (path.empty()) {
        // XXX: Branch removal not yet implemented
        throw_error(sys::errc::operation_not_permitted);
    }

    apply(branch,
        [&] (LocalBranch& b) {
            b.remove(path);
        },
        [&] (RemoteBranch&) {
            throw_error(sys::errc::operation_not_permitted);
        });

    co_return;
}

net::awaitable<size_t> FileSystem::truncate(PathRange path, size_t size)
{
    if (path.empty()) {
        // XXX: Branch removal not yet implemented
        throw_error(sys::errc::is_a_directory);
    }

    auto& branch = find_branch(path);
    path.advance_begin(1);

    if (path.empty()) {
        // XXX: Branch removal not yet implemented
        throw_error(sys::errc::is_a_directory);
    }

    co_return apply(branch,
        [&] (LocalBranch& b) -> size_t {
            return b.truncate(path, size);
        },
        [&] (RemoteBranch&) {
            throw_error(sys::errc::operation_not_permitted);
            return 0; // Satisfy warning
        });
}

/* static */
const VersionVector& FileSystem::get_version_vector(const Branch& b)
{
    return *apply(b, [] (auto& b) { return &b.version_vector(); });
}

object::Id FileSystem::get_root_id(const Branch& b)
{
    return apply(b, [] (auto& b) { return b.root_object_id(); });
}

RemoteBranch*
FileSystem::get_or_create_remote_branch(const UserId& user_id, const object::Id& root, const VersionVector& vv)
{
    for (auto& [_, branch] : _branches) {
        (void) _;

        if (root == get_root_id(branch))
            return boost::get<RemoteBranch>(&branch);

        if (vv <= get_version_vector(branch))
            return nullptr;
    }

    auto path = _options.remotes / user_id.to_string();

    auto [i, inserted] = _branches.insert(std::make_pair(user_id,
                RemoteBranch(user_id, root, vv, path, _options.objectdir)));

    assert(inserted);

    return boost::get<RemoteBranch>(&i->second);
}
