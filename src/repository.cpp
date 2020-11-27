#include "repository.h"
#include "snapshot.h"
#include "branch_io.h"
#include "branch_type.h"
#include "random.h"
#include "hex.h"

#include <iostream>
#include <boost/filesystem.hpp>
#include <boost/range/iterator_range.hpp>

#include <boost/archive/binary_oarchive.hpp>
#include <boost/archive/binary_iarchive.hpp>
#include <boost/serialization/vector.hpp>

using namespace ouisync;
using std::vector;
using std::map;
using std::string;
using boost::get;
using std::make_pair;
using Branch = Repository::Branch;

static
Branch _load_branch(const fs::path& path, const fs::path& objdir)
{
    fs::fstream file(path, file.binary | file.in);
    boost::archive::binary_iarchive archive(file);

    BranchType type;

    archive >> type;

    assert(type == BranchType::Local || type == BranchType::Remote);

    if (type == BranchType::Local) {
        return LocalBranch(path, objdir, archive);
    } else {
        return RemoteBranch(path, objdir, archive);
    }
}

Repository::Repository(executor_type ex, Options options) :
    _ex(std::move(ex)),
    _options(std::move(options)),
    _on_change(_ex)
{
    _user_id = UserId::load_or_create(_options.user_id_file_path);

    for (auto f : fs::directory_iterator(_options.branchdir)) {
        auto branch = _load_branch(f, _options.objectdir);
        auto user_id = UserId::from_string(f.path().filename().native());
        if (!user_id) {
            throw std::runtime_error("Repository: Invalid branch name format");
        }
        _branches.insert(make_pair(*user_id, std::move(branch)));
    }

    if (_branches.count(_user_id) == 0) {
        fs::path path = _options.branchdir / _user_id.to_string();
        _branches.insert(make_pair(_user_id,
                    LocalBranch::create(path, _options.objectdir, _user_id)));
    }
}

Branch& Repository::find_branch(PathRange path)
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
        apply(b, [] (const auto& b) { return b.stamp(); }),
        apply(b, [] (const auto& b) { return b.root_id(); })
    };
}

static BranchIo::Immutable _immutable_io(const Branch& b) {
    return apply(b, [&] (auto& b) { return b.immutable_io(); });
}

SnapshotGroup Repository::create_snapshot_group()
{
    std::map<UserId, Snapshot> snapshots;

    for (auto& [user_id, branch] : _branches) {
        auto snapshot = Snapshot::create(
                _options.snapshotdir, _options.objectdir, _get_commit(branch));

        snapshots.insert(std::make_pair(user_id, std::move(snapshot)));
    }

    SnapshotGroup group(std::move(snapshots));
    _last_snapshot_id = group.id();
    return group;
}

net::awaitable<Repository::Attrib> Repository::get_attr(PathRange path)
{
    if (path.empty()) co_return DirAttrib{};

    auto& branch = find_branch(path);

    path.advance_begin(1);
    auto ret = _immutable_io(branch).get_attr(path);
    co_return ret;
}

net::awaitable<vector<string>> Repository::readdir(PathRange path)
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
        auto dir = _immutable_io(branch).readdir(path);

        for (auto& [name, hash] : dir) {
            nodes.push_back(name);
        }
    }

    co_return nodes;
}

net::awaitable<size_t> Repository::read(PathRange path, char* buf, size_t size, off_t offset)
{
    if (path.empty()) {
        throw_error(sys::errc::invalid_argument);
    }

    auto& branch = find_branch(path);
    path.advance_begin(1);

    if (path.empty()) {
        throw_error(sys::errc::is_a_directory);
    }

    co_return _immutable_io(branch).read(path, buf, size, offset);
}

net::awaitable<size_t> Repository::write(PathRange path, const char* buf, size_t size, off_t offset)
{
    if (path.empty()) {
        throw_error(sys::errc::invalid_argument);
    }

    auto& branch = find_branch(path);
    path.advance_begin(1);

    if (path.empty()) {
        throw_error(sys::errc::is_a_directory);
    }

    auto retval = apply(branch,
            [&] (LocalBranch& b) -> size_t {
                return b.write(path, buf, size, offset);
            },
            [&] (RemoteBranch&) -> size_t {
                throw_error(sys::errc::operation_not_permitted);
                return 0; // Satisfy warning
            });

    _on_change.notify();
    co_return retval;
}

net::awaitable<void> Repository::mknod(PathRange path, mode_t mode, dev_t dev)
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

    _on_change.notify();
    co_return;
}

net::awaitable<void> Repository::mkdir(PathRange path, mode_t mode)
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

    _on_change.notify();
    co_return;
}

net::awaitable<void> Repository::remove_file(PathRange path)
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

    _on_change.notify();
    co_return;
}

net::awaitable<void> Repository::remove_directory(PathRange path)
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

    _on_change.notify();
    co_return;
}

net::awaitable<size_t> Repository::truncate(PathRange path, size_t size)
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

    auto retval = apply(branch,
        [&] (LocalBranch& b) -> size_t {
            return b.truncate(path, size);
        },
        [&] (RemoteBranch&) {
            throw_error(sys::errc::operation_not_permitted);
            return 0; // Satisfy warning
        });

    _on_change.notify();
    co_return retval;
}

/* static */
const VersionVector& Repository::get_stamp(const Branch& b)
{
    return *apply(b, [] (auto& b) { return &b.stamp(); });
}

object::Id Repository::get_root_id(const Branch& b)
{
    return apply(b, [] (auto& b) { return b.root_id(); });
}

net::awaitable<RemoteBranch*>
Repository::get_or_create_remote_branch(const UserId& user_id, const Commit& commit)
{
    if (user_id == _user_id) {
        // XXX: In future we could also do donwload of local branches, e.g. for
        // cases when a user loses some data.
        co_return nullptr;
    }

    auto i = _branches.find(user_id);

    if (i == _branches.end()) {
        auto path = _options.remotes / user_id.to_string();

        i = _branches.insert(std::make_pair(user_id,
             RemoteBranch(commit, path, _options.objectdir))).first;

        co_return boost::get<RemoteBranch>(&i->second);
    }

    auto* branch = boost::get<RemoteBranch>(&i->second);

    assert(branch);
    if (!branch) co_return nullptr;

    if (commit.stamp <= branch->stamp()) {
        co_return nullptr;
    }

    co_await branch->introduce_commit(commit);

    co_return branch;
}
