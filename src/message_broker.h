#pragma once

#include "shortcuts.h"
#include "cancel.h"

#include <boost/asio/ip/tcp.hpp>
#include <boost/asio/awaitable.hpp>

namespace ouisync {

class FileSystem;

class MessageBroker {
public:
    MessageBroker(FileSystem&, net::ip::tcp::socket);

    net::awaitable<void> run(Cancel);

private:
    FileSystem& _fs;
    net::ip::tcp::socket _socket;
};

} // namespace
