#include "client.h"

#include <iostream>

using namespace ouisync;
using std::move;
using Branch = FileSystem::Branch;

Client::Client(MessageBroker::Client&& broker, FileSystem& fs) :
    _broker(move(broker)),
    _fs(fs)
{
    (void) _fs;
}

net::awaitable<void> Client::run(Cancel cancel)
{
    co_await _broker.send(RqHeads{}, cancel);
    auto rs = co_await _broker.receive(cancel);
    std::cerr << "Client received " << rs << "\n";

    auto* heads = boost::get<RsHeads>(&rs);
    assert(heads);

    for (auto& commit : *heads) {
        auto branch = _fs.get_or_create_remote_branch(commit.root_object_id, commit.version_vector);
        (void) branch;
    }
}
