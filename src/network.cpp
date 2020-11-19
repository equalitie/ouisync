#include "network.h"
#include "barrier.h"
#include "client.h"
#include "server.h"
#include "message_broker.h"

#include <boost/asio/detached.hpp>
#include <boost/asio/co_spawn.hpp>

#include <iostream>

using namespace ouisync;
using net::ip::tcp;
using std::move;

Network::Network(executor_type ex, Repository& repo, Options options) :
    _ex(ex),
    _repo(repo),
    _options(move(options))
{
    if (_options.accept_endpoint) {
        co_spawn(_ex, keep_accepting(*_options.accept_endpoint), net::detached);
    }

    if (_options.connect_endpoint) {
        co_spawn(_ex, connect(*_options.connect_endpoint), net::detached);
    }
}

net::awaitable<void> Network::keep_accepting(tcp::endpoint ep)
{
    Cancel cancel(_lifetime_cancel);

    tcp::acceptor acceptor(_ex, ep);

    auto close_acceptor = cancel.connect([&] { acceptor.close(); });

    try {
        while (!cancel) {
            tcp::socket socket = co_await acceptor.async_accept(net::use_awaitable);
            if (cancel) break;
            establish_communication(move(socket));
        }
    }
    catch (const std::exception& e) {
        if (!cancel) {
            std::cerr << "Error accepting: " << e.what() << "\n";
        }
    }
}

net::awaitable<void> Network::connect(tcp::endpoint ep)
{
    Cancel cancel(_lifetime_cancel);

    try {
        tcp::socket socket(_ex);
        auto close_socket = cancel.connect([&] { socket.close(); });
        co_await socket.async_connect(ep, net::use_awaitable);
        if (!cancel) establish_communication(move(socket));
    }
    catch (const std::exception& e) {
        if (!cancel) {
            std::cerr << "Error connecting: " << e.what() << "\n";
        }
    }
}

void Network::establish_communication(tcp::socket socket)
{
    Cancel cancel(_lifetime_cancel);

    co_spawn(_ex,
    [ &,
      ex = _ex,
      c = move(cancel),
      s = move(socket)
    ]
    () mutable -> net::awaitable<void> {
        if (c) co_return;

        auto ep = s.remote_endpoint();
        std::cerr << "Establishing communication with " << ep << "\n";

        Barrier b(ex);

        MessageBroker broker(ex, move(s));
        Server server(broker.server(), _repo);
        Client client(broker.client(), _repo);

        co_spawn(ex, broker.run(c), [&, l = b.lock()](auto){c();});
        co_spawn(ex, server.run(c), [&, l = b.lock()](auto){c();});
        co_spawn(ex, client.run(c), [&, l = b.lock()](auto){c();});

        co_await b.wait({});

        std::cerr << "Finished communication with " << ep << "\n";
    },
    net::detached);
}

void Network::finish()
{
    _lifetime_cancel();
}
