#include "server.h"

#include <iostream>
#include <boost/optional/optional_io.hpp>
#include <boost/serialization/vector.hpp>

using namespace ouisync;
using std::move;
using std::cerr;
using std::make_pair;

Server::Server(MessageBroker::Server&& broker, Branch& branch) :
    _broker(move(broker)),
    _branch(branch)
{
}

net::awaitable<void> Server::run(Cancel cancel)
{
    using AwaitVoid = net::awaitable<void>;

    while (true) {
        auto m = co_await _broker.receive(cancel);

        auto handle_rq_index = [&] (const RqIndex&) -> AwaitVoid {
            co_await _broker.send(RsIndex{_branch.index()}, cancel);
        };

        auto handle_rq_block = [&] (const RqBlock& rq) -> AwaitVoid {
            RsBlock rs;
            auto block = _branch.block_store().maybe_load(rq.block_id);
            co_await _broker.send({RsBlock{std::move(block)}}, cancel);
        };

        auto handle_notify_on_change = [&] (RqNotifyOnChange rq) -> AwaitVoid {
            auto new_state = co_await _branch.on_change().wait(rq.last_state, cancel);
            co_await _broker.send({RsNotifyOnChange{new_state}}, cancel);
        };

        co_await apply(m,
            [&] (const RqIndex& rq) { return handle_rq_index(rq); },
            [&] (const RqBlock& rq) { return handle_rq_block(rq); },
            [&] (const RqNotifyOnChange& rq) { return handle_notify_on_change(rq); });
    }
}
