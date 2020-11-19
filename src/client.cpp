#include "client.h"

#include <iostream>

using namespace ouisync;
using std::move;
using Branch = Repository::Branch;

Client::Client(MessageBroker::Client&& broker, Repository& repo) :
    _broker(move(broker)),
    _repo(repo)
{
    (void) _repo;
}

net::awaitable<void> Client::run(Cancel cancel)
{
    co_await _broker.send(RqHeads{}, cancel);
    auto rs = co_await _broker.receive(cancel);
    std::cerr << "Client received " << rs << "\n";

    auto* heads = boost::get<RsHeads>(&rs);
    assert(heads);

    for (auto& commit : *heads) {
        auto branch = _repo.get_or_create_remote_branch(commit);
        (void) branch;
    }
}
