#pragma once

#include "shortcuts.h"
#include "options.h"
#include "cancel.h"

#include <boost/asio/awaitable.hpp>

namespace ouisync {

class FileSystem;

class Network {
public:
    using executor_type = net::any_io_executor;

public:
    Network(executor_type, FileSystem&, Options);

    void finish();

private:
    net::awaitable<void> keep_accepting(net::ip::tcp::endpoint);
    net::awaitable<void> connect(net::ip::tcp::endpoint);

    void spawn_message_broker(net::ip::tcp::socket);

private:
    executor_type _ex;
    FileSystem& _fs;
    const Options _options;
    ScopedCancel _lifetime_cancel;
};

} // namespace
