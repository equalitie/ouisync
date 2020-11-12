#include "message_broker.h"
#include "error.h"
#include "barrier.h"

#include <boost/asio/detached.hpp>
#include <boost/asio/co_spawn.hpp>
#include <iostream>

using namespace ouisync;
using net::ip::tcp;
using std::move;
using boost::get;

//--------------------------------------------------------------------

MessageBroker::MessageBroker(executor_type ex, tcp::socket socket) :
    _ex(ex),
    _socket(move(socket)),
    _send_bouncer(_ex),
    _rx_request_bouncer(_ex),
    _rx_response_bouncer(_ex),
    _rx_pending_request(_rx_request_bouncer.lock()),
    _rx_pending_response(_rx_response_bouncer.lock())
{
}

//--------------------------------------------------------------------

net::awaitable<void> MessageBroker::Client::send(Request m, Cancel c)
{
    return broker->send(std::move(m), move(c));
}

net::awaitable<void> MessageBroker::Server::send(Response m, Cancel c)
{
    return broker->send(std::move(m), move(c));
}

net::awaitable<Response> MessageBroker::Client::receive(Cancel cancel)
{
    return broker->receive_response(cancel);
}

net::awaitable<Request> MessageBroker::Server::receive(Cancel cancel)
{
    return broker->receive_request(cancel);
}

//--------------------------------------------------------------------

net::awaitable<Response> MessageBroker::receive_response(Cancel cancel)
{
    Bouncer::Lock lock;

    if (get<Bouncer::Lock>(&_rx_pending_response)) {
        lock = co_await _rx_response_bouncer.wait(cancel);
    }

    auto mp = get<Response>(&_rx_pending_response);
    if (!mp) throw std::runtime_error("Should have had a response");

    auto m = std::move(*mp);
    _rx_pending_response = lock ? move(lock) : _rx_response_bouncer.lock();
    co_return m;
}

net::awaitable<Request> MessageBroker::receive_request(Cancel cancel)
{
    Bouncer::Lock lock;

    if (get<Bouncer::Lock>(&_rx_pending_request)) {
        lock = co_await _rx_request_bouncer.wait(cancel);
    }

    auto mp = get<Request>(&_rx_pending_request);
    if (!mp) throw std::runtime_error("Should have had a request");

    auto m = std::move(*mp);
    _rx_pending_request = lock ? move(lock) : _rx_request_bouncer.lock();
    co_return m;
}

//--------------------------------------------------------------------

net::awaitable<void> MessageBroker::send(Message message, Cancel cancel)
{
    auto send_lock = co_await _send_bouncer.wait(cancel);
    co_await Message::send(_socket, message, cancel);
}

//--------------------------------------------------------------------

net::awaitable<void> MessageBroker::handle_message(Message m, Cancel cancel)
{
    using namespace sys::errc;

    auto is_lock = [] (auto& v) -> bool {
        return get<Bouncer::Lock>(&v);
    };

    auto fill_rq = [&] (auto& m) {
        if (!is_lock(_rx_pending_request)) throw_error(protocol_error);
        _rx_pending_request = move(m);
    };

    auto fill_rs = [&] (auto& m) {
        if (!is_lock(_rx_pending_response)) throw_error(protocol_error);
        _rx_pending_response = move(m);
    };

    apply(m,
        [&] (RqBranchList& m) { fill_rq(m); },
        [&] (RqBranch& m)     { fill_rq(m); },
        [&] (RsBranchList& m) { fill_rs(m); },
        [&] (RsBranch& m)     { fill_rs(m); }
    );

    co_return;
}

//--------------------------------------------------------------------

net::awaitable<void> MessageBroker::keep_receiving(Cancel cancel)
try {
    while (true) {
        auto m = co_await Message::receive(_socket, cancel);
        co_await handle_message(std::move(m), cancel);
    }
}
catch (const sys::system_error& e) {
    if (cancel) co_return;
    std::cerr << "MessageBroker: keep_receiving stopped: "
        << e.what() << "\n";
}
catch (const std::exception& e) {
    std::cerr << "MessageBroker: Unknown std::exception in keep_receiving: "
        << e.what() << "\n";
}

//--------------------------------------------------------------------

net::awaitable<void> MessageBroker::run(Cancel cancel)
{
    co_await keep_receiving(cancel);
}

//--------------------------------------------------------------------

