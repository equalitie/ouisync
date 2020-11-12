#include "client.h"

#include <iostream>

using namespace ouisync;
using std::move;

Client::Client(MessageBroker::Client&& broker) :
    _broker(move(broker))
{}

net::awaitable<void> Client::run(Cancel cancel)
{
    std::cerr << "Client " << __LINE__ << "\n";
    co_await _broker.send(RqBranchList{}, cancel);
    std::cerr << "Client " << __LINE__ << " " << bool(cancel) << "\n";
    auto rs = co_await _broker.receive(cancel);

    std::cerr << "Client received " << rs << "\n";
}
