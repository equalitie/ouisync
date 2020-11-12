#pragma once

#include "message_broker.h"

namespace ouisync {

class Client {
public:
    Client(MessageBroker::Client&&);

    net::awaitable<void> run(Cancel);

private:
    MessageBroker::Client _broker;
};

} // namespace
