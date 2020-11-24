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

/* static */
Repository::HexBranchId Repository::generate_branch_id()
{
    static constexpr size_t hex_size = std::tuple_size<HexBranchId>::value;
    static_assert(hex_size % 2 == 0, "Hex size must be an even number");
    std::array<uint8_t, hex_size/2> bin;
    random::generate_non_blocking(bin.data(), bin.size());
    return to_hex<char>(bin);
}

Opt<Repository::HexBranchId> Repository::str_to_branch_id(const std::string& name)
{
    if (name.size() != std::tuple_size<HexBranchId>::value)
        return boost::none;

    // XXX: Check that all letters are hex

    HexBranchId bid;
    for (size_t i = 0; i < name.size(); ++i) bid[i] = name[i];
    return bid;
}

Repository::Repository(executor_type ex, Options options) :
    _ex(std::move(ex)),
    _options(std::move(options)),
    _on_change(_ex)
{
    _user_id = UserId::load_or_create(_options.user_id_file_path);

    bool local_exists = false;

    for (auto f : fs::directory_iterator(_options.branchdir)) {
        auto branch = _load_branch(f, _options.objectdir);
        auto branch_id = str_to_branch_id(f.path().filename().native());
        if (!branch_id) {
            throw std::runtime_error("Repository: Invalid branch name format");
        }
        local_exists |= bool(boost::get<LocalBranch>(&branch));
        _branches.insert(make_pair(*branch_id, std::move(branch)));
    }

    if (!local_exists) {
        auto branch_id = generate_branch_id();
        fs::path path = _options.branchdir / std::string(branch_id.begin(), branch_id.end());
        _branches.insert(make_pair(branch_id,
                    LocalBranch::create(path, _options.objectdir, _user_id)));
    }
}

Branch& Repository::find_branch(PathRange path)
{
    if (path.empty()) throw_error(sys::errc::invalid_argument);
    auto branch_id = str_to_branch_id(path.front().native());
    if (!branch_id) throw_error(sys::errc::invalid_argument);
    auto i = _branches.find(*branch_id);
    if (i == _branches.end()) throw_error(sys::errc::invalid_argument);
    return i->second;
}

static Commit _get_commit(const Branch& b) {
    return {
        apply(b, [] (const auto& b) { return b.version_vector(); }),
        apply(b, [] (const auto& b) { return b.root_object_id(); })
    };
}

static BranchIo::Immutable _immutable_io(const Branch& b) {
    return apply(b, [&] (auto& b) { return b.immutable_io(); });
}

Snapshot Repository::create_snapshot()
{
    Snapshot::Commits commits;

    for (auto& [branch_id, branch] : _branches) {
        (void) branch_id;
        commits.insert(_get_commit(branch));
    }

    auto snapshot = Snapshot::create(_options.snapshotdir, _options.objectdir, std::move(commits));
    _last_snapshot_id = snapshot.id();
    return snapshot;
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
            nodes.push_back(std::string(name.begin(), name.end()));
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
const VersionVector& Repository::get_version_vector(const Branch& b)
{
    return *apply(b, [] (auto& b) { return &b.version_vector(); });
}

object::Id Repository::get_root_id(const Branch& b)
{
    return apply(b, [] (auto& b) { return b.root_object_id(); });
}

RemoteBranch*
Repository::get_or_create_remote_branch(const Commit& commit)
{
    for (auto& [_, branch] : _branches) {
        (void) _;

        if (commit.root_object_id == get_root_id(branch))
            return boost::get<RemoteBranch>(&branch);

        if (commit.version_vector <= get_version_vector(branch))
            return nullptr;
    }

    auto branch_id = generate_branch_id();
    auto path = _options.remotes / std::string(branch_id.begin(), branch_id.end());

    auto [i, inserted] = _branches.insert(std::make_pair(branch_id,
                RemoteBranch(commit, path, _options.objectdir)));

    assert(inserted);

    return boost::get<RemoteBranch>(&i->second);
}
