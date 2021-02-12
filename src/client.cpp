#include "client.h"
#include "error.h"
#include "directory.h"
#include "file_blob.h"

#include <iostream>
#include <boost/optional/optional_io.hpp>

using namespace ouisync;
using std::move;

Client::Client(MessageBroker::Client&& broker, Branch& branch) :
    _broker(move(broker)),
    _branch(branch)
{}

template<class T>
net::awaitable<T> Client::receive(Cancel cancel)
{
    auto rs = co_await _broker.receive(cancel);
    auto t = boost::get<T>(&rs);
    if (!t) throw_error(sys::errc::protocol_error);
    co_return std::move(*t);
}

net::awaitable<void> Client::run(Cancel cancel)
{
    uint64_t state_counter = 0;

    while (true) {
        co_await _broker.send(RqIndices{}, cancel);
        auto indices = co_await receive<RsIndices>(cancel);

        std::cerr << "got indices\n";

        co_await _broker.send(RqNotifyOnChange{state_counter}, cancel);
        auto rs_on_change = co_await receive<RsNotifyOnChange>(cancel);
        state_counter = rs_on_change.new_state;
    }
}
