#pragma once

#include "shortcuts.h"
#include "cancel.h"

#include <boost/asio/ip/tcp.hpp>
#include <boost/asio/awaitable.hpp>

namespace ouisync {

class FileSystem;

class Dealer {
public:
    Dealer(FileSystem&, net::ip::tcp::socket);

    net::awaitable<void> run(Cancel);

public:
    FileSystem& _fs;
    net::ip::tcp::socket _socket;
    ScopedCancel _lifetime_cancel;
};

} // namespace
