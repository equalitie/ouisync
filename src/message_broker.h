#pragma once

#include "shortcuts.h"
#include "bouncer.h"
#include "message.h"
#include "variant.h"

#include <boost/asio/ip/tcp.hpp>
#include <boost/asio/awaitable.hpp>

namespace ouisync {

struct Message;

/*
 * Each node acts as a client and as a server, MessageBroker helps split
 * messages that are meant for the client and for the server.  It also protects
 * from using one socket for sending from two different coroutines at once.
 *
 * This way two different coroutines can independently act at as client and a
 * server.
 */
class MessageBroker {
public:
    using executor_type = net::any_io_executor;

    class Client {
        public:
        net::awaitable<void> send(Request, Cancel);
        net::awaitable<Response> receive(Cancel);
        private:
        friend class MessageBroker;
        Client(MessageBroker* b) : broker(b) {}
        MessageBroker* broker;
    };

    class Server {
        public:
        net::awaitable<Request> receive(Cancel);
        net::awaitable<void> send(Response, Cancel);
        private:
        friend class MessageBroker;
        Server(MessageBroker* b) : broker(b) {}
        MessageBroker* broker;
    };

public:
    MessageBroker(executor_type, net::ip::tcp::socket);

    /*
     * Note: The assumption is that at most one of each {Client,Server} is
     * created. Or if more (e.g. Clients) are created, then one must ensure
     * that their send/receive methods are not used in concurrently. 
     */
    Client client() { return Client(this); }
    Server server() { return Server(this); } 

    net::awaitable<void> run(Cancel);

private:
    net::awaitable<void> keep_receiving(Cancel);
    net::awaitable<void> send(Message, Cancel);
    net::awaitable<void> handle_message(Message, Cancel);

    net::awaitable<Request> receive_request(Cancel);
    net::awaitable<Response> receive_response(Cancel);

private:
    executor_type _ex;
    net::ip::tcp::socket _socket;
    Bouncer _send_bouncer;
    Bouncer _rx_request_bouncer;
    Bouncer _rx_response_bouncer;
    variant<Bouncer::Lock, Request> _rx_pending_request;
    variant<Bouncer::Lock, Response> _rx_pending_response;
};

} // namespace
