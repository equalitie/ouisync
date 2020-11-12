#pragma once

#include "message_broker.h"

namespace ouisync {

class Server {
public:
    Server(MessageBroker::Server&&);

    net::awaitable<void> run(Cancel);

private:
    MessageBroker::Server _broker;
};

} // namespace
