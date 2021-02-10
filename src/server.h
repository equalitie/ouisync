#pragma once

#include "message_broker.h"
#include "repository.h"

namespace ouisync {

class Server {
public:
    Server(MessageBroker::Server&&, Repository&);

    net::awaitable<void> run(Cancel);

private:
    MessageBroker::Server _broker;
};

} // namespace
