#include "server.h"
#include "snapshot.h"

#include <iostream>
#include <boost/optional/optional_io.hpp>

using namespace ouisync;
using std::move;

Server::Server(MessageBroker::Server&& broker, Repository& repo) :
    _broker(move(broker)),
    _repo(repo)
{
}

net::awaitable<void> Server::run(Cancel cancel)
{
    using AwaitVoid = net::awaitable<void>;

    Opt<Snapshot> snapshot;

    while (true) {
        auto m = co_await _broker.receive(cancel);

        std::cerr << "Server received " << m << "   Snapshot:" << snapshot << "\n";

        auto handle_rq_snapshot = [&] (const RqSnapshot& rq) -> AwaitVoid {
            snapshot = _repo.create_snapshot();

            std::cerr << "Server created snapshot 1: " << *snapshot << "\n";
            for (auto commit : snapshot->commits()) {
                BranchIo::Immutable(_repo.object_directory(), commit.root_object_id).show(std::cerr);
            }

            if (rq.last_snapshot_id &&
                    snapshot->id() == *rq.last_snapshot_id) {
                std::cerr << "Server waiting for a change\n";
                co_await _repo.on_change().wait(cancel);
                std::cerr << "done\n";
            }

            RsSnapshot rsp;
            rsp.snapshot_id = snapshot->id();
            rsp.reserve(snapshot->commits().size());

            for (auto& c : snapshot->commits()) {
                rsp.push_back(c);
            }

            co_await _broker.send(move(rsp), cancel);
        };

        auto handle_rq_object = [&] (const RqObject& rq) -> AwaitVoid {
            if (!snapshot) {
                snapshot = _repo.create_snapshot();
                std::cerr << "Server created snapshot 2: " << *snapshot << "\n";
            }

            RsObject rs;
            auto object = snapshot->load_object(rq.object_id);

            co_await _broker.send({RsObject{std::move(object)}}, cancel);
        };

        co_await apply(m,
            [&] (const RqSnapshot& rq) { return handle_rq_snapshot(rq); },
            [&] (const RqObject& rq) { return handle_rq_object(rq); });
    }
}
