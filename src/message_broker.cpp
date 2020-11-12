#include "message_broker.h"
#include "error.h"
#include "message.h"

#include <iostream>

using namespace ouisync;
using net::ip::tcp;
using std::move;

MessageBroker::MessageBroker(FileSystem& fs, tcp::socket socket) :
    _fs(fs),
    _socket(move(socket))
{
    (void) _fs;
}

net::awaitable<void> MessageBroker::run(Cancel cancel)
{
    std::cerr << "Going to communicate with " << _socket.remote_endpoint() << "\n";
    co_await Message::send(_socket, {RqBranchList{}}, cancel);
    auto m = co_await Message::receive(_socket, cancel);
    std::cerr << "Received " << m << "\n";

    co_return;
}
