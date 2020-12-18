#include "repository.h"
#include "snapshot.h"
#include "branch_io.h"
#include "branch_type.h"
#include "random.h"
#include "hex.h"
#include "archive.h"
#include "defer.h"

#include <iostream>
#include <boost/filesystem.hpp>
#include <boost/range/iterator_range.hpp>

#include <boost/serialization/vector.hpp>

using namespace ouisync;
using std::vector;
using std::map;
using std::string;
using boost::get;
using std::make_pair;
using Branch = Repository::Branch;

#define SANITY_CHECK_EACH_IO_FN false

static void _sanity_check(const Branch& b) {
    apply(b, [&] (auto& b) { b.sanity_check(); });
}

Repository::Repository(executor_type ex, Options options) :
    _ex(std::move(ex)),
    _options(std::move(options)),
    _on_change(_ex)
{
    _user_id = UserId::load_or_create(_options.user_id_file_path);

    for (auto f : fs::directory_iterator(_options.branchdir)) {
        auto user_id = UserId::from_string(f.path().filename().string());
        if (!user_id) {
            throw std::runtime_error("Repository: Invalid branch name format");
        }
        auto branch = RemoteBranch::load(f, _options);
        _branches.insert(make_pair(*user_id, std::move(branch)));
    }

    fs::path local_branch_path = _options.basedir / "local_branch";

    if (fs::exists(local_branch_path)) {
        fs::path path = _options.branchdir / _user_id.to_string();
        _branches.insert(make_pair(_user_id,
                    LocalBranch::load(path, _user_id, _options)));
    } else {
        _branches.insert(make_pair(_user_id,
                    LocalBranch::create(local_branch_path, _user_id, _options)));
    }

    std::cout << "User ID: " << _user_id << "\n";
}

Branch& Repository::find_branch(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::invalid_argument);
    auto user_id = UserId::from_string(path.front());
    if (!user_id) throw_error(sys::errc::invalid_argument);
    auto i = _branches.find(*user_id);
    if (i == _branches.end()) throw_error(sys::errc::invalid_argument);
    return i->second;
}

static BranchIo::Immutable _immutable_io(const Branch& b) {
    return apply(b, [&] (auto& b) { return b.immutable_io(); });
}

static Snapshot _create_snapshot(const Branch& b, const fs::path& snapshotdir) {
    return apply(b, [&] (auto& b) { return b.create_snapshot(); });
}

SnapshotGroup Repository::create_snapshot_group()
{
#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

    std::map<UserId, Snapshot> snapshots;

    for (auto& [user_id, branch] : _branches) {
        snapshots.insert({user_id, _create_snapshot(branch, _options.snapshotdir)});
    }

    SnapshotGroup group(std::move(snapshots));
    _last_snapshot_id = group.id();
    return group;
}

net::awaitable<Repository::Attrib> Repository::get_attr(PathRange path)
{
#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

    if (path.empty()) co_return DirAttrib{};

    auto& branch = find_branch(path);

    path.advance_begin(1);
    auto ret = _immutable_io(branch).get_attr(path);
    co_return ret;
}

net::awaitable<vector<string>> Repository::readdir(PathRange path)
{
#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

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
#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

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
#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

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
#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

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
#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

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
#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

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
#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

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
#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

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

ObjectId Repository::get_root_id(const Branch& b)
{
    return apply(b, [] (auto& b) { return b.root_id(); });
}

net::awaitable<RemoteBranch*>
Repository::get_or_create_remote_branch(const UserId& user_id, const Commit& commit)
{
#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

    if (user_id == _user_id) {
        // XXX: In future we could also do donwload of local branches, e.g. for
        // cases when a user loses some data.
        co_return nullptr;
    }

    auto i = _branches.find(user_id);

    if (i == _branches.end()) {
        auto path = _options.remotes / user_id.to_string();

        i = _branches.insert(std::make_pair(user_id,
             RemoteBranch(commit, path, _options))).first;

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

void Repository::introduce_commit_to_local_branch(const Commit& commit)
{
#if SANITY_CHECK_EACH_IO_FN
    sanity_check();
    auto at_exit = defer([&] { sanity_check(); });
#endif

    auto i = _branches.find(_user_id);
    if (i == _branches.end()) {
        throw std::runtime_error("Local branch doesn't exist");
    }

    auto b = boost::get<LocalBranch>(&i->second);

    if (!b) {
        throw std::runtime_error("Branch is not local");
    }

    if (b->introduce_commit(commit)) {
        _on_change.notify();
    }
}

void Repository::sanity_check() const
{
    for (auto& [_, branch] : _branches) {
        _sanity_check(branch);
    }
}
