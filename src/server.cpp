#include "server.h"

#include <iostream>
#include <boost/optional/optional_io.hpp>
#include <boost/serialization/vector.hpp>

using namespace ouisync;
using std::move;
using std::make_pair;
using Object = variant<FileBlob, Directory>;

Server::Server(MessageBroker::Server&& broker, const Branch& branch) :
    _broker(move(broker)),
    _branch(branch)
{
}

net::awaitable<void> Server::run(Cancel cancel)
{
    using AwaitVoid = net::awaitable<void>;

    while (true) {
        auto m = co_await _broker.receive(cancel);

        auto handle_rq_indices = [&] (const RqIndices&) -> AwaitVoid {
            RsIndices rsp{ _branch.indices() };
            co_await _broker.send(move(rsp), cancel);
        };

        auto handle_rq_object = [&] (const RqObject& rq) -> AwaitVoid {
            RsObject rs;
            auto object = _branch.objstore().load<FileBlob,Directory>(rq.object_id);

            co_await _broker.send({RsObject{std::move(object)}}, cancel);
        };

        co_await apply(m,
            [&] (const RqIndices& rq) { return handle_rq_indices(rq); },
            [&] (const RqObject& rq) { return handle_rq_object(rq); });
    }
}
