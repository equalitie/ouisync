#pragma once

#include "message_broker.h"
#include "repository.h"

namespace ouisync {

class RemoteBranch;

class Client {
private:
    using Id = object::Id;

public:
    Client(MessageBroker::Client&&, Repository&);

    net::awaitable<void> run(Cancel);

private:
    template<class T> net::awaitable<T> receive(Cancel);
    net::awaitable<void> download_branch(RemoteBranch&, Id, Cancel);

private:
    MessageBroker::Client _broker;
    Repository& _repo;
};

} // namespace
