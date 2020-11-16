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

static
VersionVector _get_version_vector(const Branch& b)
{
    return apply(b, [] (auto& b) { return b.version_vector(); });
}

template<class Where>
static
bool _is_contained_in(const Commit& what, Where& where)
{
    for (auto& [user_id, branch] : where) {
        (void) user_id;
        if (what.version_vector <= _get_version_vector(branch))
            return true;
    }
    return false;
}

net::awaitable<void> Client::run(Cancel cancel)
{
    co_await _broker.send(RqHeads{}, cancel);
    auto rs = co_await _broker.receive(cancel);
    std::cerr << "Client received " << rs << "\n";

    auto* heads = boost::get<RsHeads>(&rs);
    assert(heads);

    for (auto& commit : *heads) {
        if (_is_contained_in(commit, _fs.branches()))
            continue;
    }
}
