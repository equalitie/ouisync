#pragma once

#include "message_broker.h"
#include "branch.h"

namespace ouisync {

class RemoteBranch;

class Client {
public:
    Client(MessageBroker::Client&&, Branch&);

    net::awaitable<void> run(Cancel);

private:
    template<class T> net::awaitable<T> receive(Cancel);

private:
    MessageBroker::Client _broker;
    Branch& _branch;
};

} // namespace
