#pragma once

#include "message_broker.h"
#include "branch.h"

namespace ouisync {

class Server {
public:
    Server(MessageBroker::Server&&, Branch&);

    net::awaitable<void> run(Cancel);

private:
    MessageBroker::Server _broker;
    Branch& _branch;
};

} // namespace
