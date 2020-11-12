#include "network.h"
#include "message_broker.h"

#include <boost/asio/detached.hpp>
#include <boost/asio/co_spawn.hpp>

#include <iostream>

using namespace ouisync;
using net::ip::tcp;
using std::move;

Network::Network(executor_type ex, FileSystem& fs, Options options) :
    _ex(ex),
    _fs(fs),
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
            spawn_message_broker(move(socket));
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
        if (!cancel) spawn_message_broker(move(socket));
    }
    catch (const std::exception& e) {
        if (!cancel) {
            std::cerr << "Error connecting: " << e.what() << "\n";
        }
    }
}

void Network::spawn_message_broker(tcp::socket socket)
{
    Cancel cancel(_lifetime_cancel);

    co_spawn(_ex,
    [ &,
      c = move(cancel),
      s = move(socket)
    ]
    () mutable -> net::awaitable<void> {
        try {
            if (c) co_return;
            MessageBroker broker(_fs, move(s));
            co_await broker.run(move(c));
        }
        catch (const std::exception& e) {
            std::cerr << "MessageBroker finished with an exception: "
                << e.what() << "\n";
        }
        co_return;
    },
    net::detached);
}

void Network::finish()
{
    _lifetime_cancel();
}
