#include "network.h"
#include "barrier.h"
#include "client.h"
#include "server.h"
#include "message_broker.h"
#include "repository.h"

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

namespace {
    struct FirstError {
        const char* whence;
        std::string message;
    };

    struct OnExit {
        Cancel& cancel;
        const char* whence;
        Barrier::Lock lock;
        Opt<FirstError>& first_error;

        void operator() (std::exception_ptr eptr) {
            if (first_error) return;
            cancel();
            first_error = FirstError{whence, {}};
            if (eptr) {
                try {
                    std::rethrow_exception(eptr);
                } catch (const std::exception& ex) {
                    first_error->message = ex.what();
                }
            }
        }
    };
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
        Server server(broker.server(), _repo.branch());
        Client client(broker.client(), _repo.branch());

        Opt<FirstError> first_error;

        co_spawn(ex, broker.run(c), OnExit{c, "broker", b.lock(), first_error});
        co_spawn(ex, server.run(c), OnExit{c, "server", b.lock(), first_error});
        co_spawn(ex, client.run(c), OnExit{c, "client", b.lock(), first_error});

        co_await b.wait({});

        std::cerr << "Finished communication with " << ep << "\n";

        assert(first_error);

        if (first_error) {
            std::cerr << "    " << first_error->whence << ": " << first_error->message << "\n";
        }
    },
    net::detached);
}

void Network::finish()
{
    _lifetime_cancel();
}
