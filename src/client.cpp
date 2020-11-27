#include "client.h"
#include "object/tree.h"
#include "object/blob.h"

#include <iostream>
#include <boost/optional/optional_io.hpp>

using namespace ouisync;
using std::move;
using Branch = Repository::Branch;
using Tree = object::Tree;
using Blob = object::Blob;

Client::Client(MessageBroker::Client&& broker, Repository& repo) :
    _broker(move(broker)),
    _repo(repo)
{}

template<class T>
net::awaitable<T> Client::receive(Cancel cancel)
{
    auto rs = co_await _broker.receive(cancel);
    auto t = boost::get<T>(&rs);
    if (!t) throw_error(sys::errc::protocol_error);
    co_return std::move(*t);
}

net::awaitable<void>
Client::download_branch(RemoteBranch& branch, ObjectId object_id, Cancel cancel)
{
    using AwaitVoid = net::awaitable<void>;

    co_await _broker.send(RqObject{object_id}, cancel);
    auto rs = co_await receive<RsObject>(cancel);

    auto handle_tree = [&] (const Tree& tree) -> AwaitVoid {
        assert(object_id == tree.calculate_id());
        ObjectId id = co_await branch.insert_tree(tree);
        assert(id == object_id);

        for (auto& [child_name, child_id] : tree) {
            (void) child_name;

            if (branch.immutable_io().object_exists(child_id))
                continue;

            co_await download_branch(branch, child_id, cancel);
        }
    };

    auto handle_blob = [&] (const Blob& blob) -> AwaitVoid {
        assert(object_id == blob.calculate_id());
        co_await branch.insert_blob(blob);
    };

    co_await apply(rs.object,
        [&] (const Tree& m) { return handle_tree(m); },
        [&] (const Blob& m) { return handle_blob(m); });
}

net::awaitable<void> Client::run(Cancel cancel)
{
    Opt<SnapshotGroup::Id> last_snapshot_group_id;

    while (true) {
        co_await _broker.send(RqSnapshotGroup{last_snapshot_group_id}, cancel);
        auto snapshot_group = co_await receive<RsSnapshotGroup>(cancel);

        last_snapshot_group_id = snapshot_group.snapshot_group_id;

        for (auto& [user_id, commit] : snapshot_group) {
            auto branch = co_await _repo.get_or_create_remote_branch(user_id, commit);

            if (!branch) {
                continue;
            }

            co_await download_branch(*branch, branch->root_id(), cancel);

            _repo.introduce_commit_to_local_branch(commit);
        }
    }
}
