#include "client.h"

#include <iostream>

using namespace ouisync;
using std::move;

Client::Client(MessageBroker::Client&& broker, FileSystem& fs) :
    _broker(move(broker)),
    _fs(fs)
{
    (void) _fs;
}

net::awaitable<void> Client::run(Cancel cancel)
{
    co_await _broker.send(RqHeads{}, cancel);
    auto rs = co_await _broker.receive(cancel);
    std::cerr << "Client received " << rs << "\n";
}
