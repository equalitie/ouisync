#include "server.h"
#include "snapshot.h"
#include "object/io.h"

#include <iostream>
#include <boost/optional/optional_io.hpp>
#include <boost/serialization/vector.hpp>

using namespace ouisync;
using std::move;
using std::make_pair;
using object::Blob;
using object::Tree;
using Object = variant<Blob, Tree>;

Server::Server(MessageBroker::Server&& broker, Repository& repo) :
    _broker(move(broker)),
    _repo(repo)
{
}

net::awaitable<void> Server::run(Cancel cancel)
{
    using AwaitVoid = net::awaitable<void>;

    Opt<SnapshotGroup> snapshot_group;

    while (true) {
        auto m = co_await _broker.receive(cancel);

        auto handle_rq_snapshot_group = [&] (const RqSnapshotGroup& rq) -> AwaitVoid {
            snapshot_group.emplace(_repo.create_snapshot_group());

            if (rq.last_snapshot_group_id &&
                    snapshot_group->id() == *rq.last_snapshot_group_id) {
                co_await _repo.on_change().wait(cancel);
            }

            RsSnapshotGroup rsp;
            rsp.snapshot_group_id = snapshot_group->id();

            for (auto& [user_id, snapshot] : *snapshot_group) {
                rsp.insert(make_pair(user_id, snapshot.commit()));
            }

            co_await _broker.send(move(rsp), cancel);
        };

        auto handle_rq_object = [&] (const RqObject& rq) -> AwaitVoid {
            if (!snapshot_group) {
                throw_error(sys::errc::protocol_error);
            }

            RsObject rs;
            // XXX Test that rq.object_id belongs to this snapshot_group
            auto object = object::io::load<Blob,Tree>(_repo.object_directory(), rq.object_id);

            co_await _broker.send({RsObject{std::move(object)}}, cancel);
        };

        co_await apply(m,
            [&] (const RqSnapshotGroup& rq) { return handle_rq_snapshot_group(rq); },
            [&] (const RqObject& rq) { return handle_rq_object(rq); });
    }
}
