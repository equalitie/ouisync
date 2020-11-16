#pragma once

#include "message_broker.h"
#include "file_system.h"

namespace ouisync {

class Client {
public:
    Client(MessageBroker::Client&&, FileSystem&);

    net::awaitable<void> run(Cancel);

private:
    MessageBroker::Client _broker;
    FileSystem& _fs;
};

} // namespace
