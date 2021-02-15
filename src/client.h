#pragma once

#include "message_broker.h"
#include "branch.h"

namespace ouisync {

class RemoteBranch;

class Client {
private:
    using ServerStateCounter = decltype(RsNotifyOnChange::new_state);

public:
    Client(MessageBroker::Client&&, Branch&);

    net::awaitable<void> run(Cancel);

private:
    template<class T> net::awaitable<T> receive(Cancel);

    net::awaitable<Branch::Indices> fetch_indices(Cancel);
    net::awaitable<Opt<RsObject::Object>> fetch_object(const ObjectId&, Cancel);
    net::awaitable<void> wait_for_a_change(Cancel);

private:
    MessageBroker::Client _broker;
    Branch& _branch;
    Opt<ServerStateCounter> _last_server_state;
};

} // namespace
