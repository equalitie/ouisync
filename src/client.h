#pragma once

#include "message_broker.h"
#include "repository.h"

namespace ouisync {

class Client {
public:
    Client(MessageBroker::Client&&, Repository&);

    net::awaitable<void> run(Cancel);

private:
    MessageBroker::Client _broker;
    Repository& _repo;
};

} // namespace
