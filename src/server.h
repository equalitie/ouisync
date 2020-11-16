#pragma once

#include "message_broker.h"
#include "file_system.h"

namespace ouisync {

class Server {
public:
    Server(MessageBroker::Server&&, FileSystem&);

    net::awaitable<void> run(Cancel);

private:
    MessageBroker::Server _broker;
    FileSystem& _fs;
};

} // namespace
