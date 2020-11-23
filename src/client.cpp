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
Client::download_branch(RemoteBranch& branch, Id object_id, Cancel cancel)
{
    using AwaitVoid = net::awaitable<void>;

    co_await _broker.send(RqObject{object_id}, cancel);
    auto rs = co_await receive<RsObject>(cancel);

    auto handle_tree = [&] (const Tree& tree) -> AwaitVoid {
        assert(object_id == object::calculate_id(tree));
        Id id = co_await branch.insert_tree(tree);
        assert(id == object_id);

        for (auto& [child_name, child_id] : tree) {
            (void) child_name;
            co_await download_branch(branch, child_id, cancel);
        }
    };

    auto handle_blob = [&] (const Blob& blob) -> AwaitVoid {
        assert(object_id == object::calculate_id(blob));
        co_await branch.insert_blob(blob);
    };

    co_await apply(rs.object,
        [&] (const Tree& m) { return handle_tree(m); },
        [&] (const Blob& m) { return handle_blob(m); });

    co_await branch.mark_complete(object_id);
}

net::awaitable<void> Client::run(Cancel cancel)
{
    Opt<Snapshot::Id> last_snapshot_id;

    while (true) {
        co_await _broker.send(RqSnapshot{last_snapshot_id}, cancel);
        auto rs = co_await _broker.receive(cancel);

        auto* heads = boost::get<RsSnapshot>(&rs);
        assert(heads);
        last_snapshot_id = heads->snapshot_id;

        for (auto& commit : *heads) {
            auto branch = _repo.get_or_create_remote_branch(commit);

            if (!branch) {
                continue;
            }

            co_await download_branch(*branch, branch->root_object_id(), cancel);
        }
    }
}
