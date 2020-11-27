#pragma once

#include "message_broker.h"
#include "repository.h"

namespace ouisync {

class RemoteBranch;

class Client {
public:
    Client(MessageBroker::Client&&, Repository&);

    net::awaitable<void> run(Cancel);

private:
    template<class T> net::awaitable<T> receive(Cancel);
    net::awaitable<void> download_branch(RemoteBranch&, ObjectId, Cancel);

private:
    MessageBroker::Client _broker;
    Repository& _repo;
};

} // namespace
