#pragma once

#include "message_broker.h"
#include "branch.h"

namespace ouisync {

class Server {
public:
    Server(MessageBroker::Server&&, const Branch&);

    net::awaitable<void> run(Cancel);

private:
    MessageBroker::Server _broker;
    const Branch& _branch;
};

} // namespace
