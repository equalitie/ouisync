#include "server.h"
#include "snapshot.h"

#include <iostream>

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

        std::cerr << "Server received " << m << "\n";

        auto handle_rq_heads = [&] () -> AwaitVoid {
            if (!snapshot) {
                snapshot = _repo.create_snapshot();
            }

            RsHeads rsp;
            rsp.reserve(snapshot->commits().size());

            for (auto& c : snapshot->commits()) {
                rsp.push_back(c);
            }

            co_await _broker.send(move(rsp), cancel);
            std::cerr << "Server sent\n";
        };

        co_await apply(m,
            [&] (const RqHeads&) { return handle_rq_heads(); });
    }
}
