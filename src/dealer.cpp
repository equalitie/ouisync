#include "dealer.h"

#include <iostream>

using namespace ouisync;
using std::move;

Dealer::Dealer(FileSystem& fs, net::ip::tcp::socket socket) :
    _fs(fs),
    _socket(move(socket))
{
}

net::awaitable<void> Dealer::run(Cancel cancel)
{
    std::cerr << "Going to communicate with " << _socket.remote_endpoint() << "\n";
    co_return;
}
