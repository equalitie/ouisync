#include "server.h"

#include <iostream>

using namespace ouisync;
using std::move;

Server::Server(MessageBroker::Server&& broker) :
    _broker(move(broker))
{}

net::awaitable<void> Server::run(Cancel cancel)
{
    while (true) {
        auto m = co_await _broker.receive(cancel);
        std::cerr << "Server received " << m << "\n";
        co_await _broker.send(RsBranchList{}, cancel);
        std::cerr << "Server sent\n";
    }
}
