#include "client.h"
#include "error.h"

#include "ostream/set.h"

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

net::awaitable<Index> Client::fetch_index(Cancel cancel)
{
    co_await _broker.send(RqIndex{}, cancel);
    auto rs = co_await receive<RsIndex>(cancel);
    co_return move(rs.index);
}

net::awaitable<Opt<Block>> Client::fetch_block(const BlockId& id, Cancel cancel)
{
    co_await _broker.send(RqBlock{id}, cancel);
    auto rs_obj = co_await receive<RsBlock>(cancel);
    co_return std::move(rs_obj.block);
}

net::awaitable<void> Client::wait_for_a_change(Cancel cancel)
{
    while (true) {
        co_await _broker.send(RqNotifyOnChange{_last_server_state}, cancel);
        auto rs_on_change = co_await receive<RsNotifyOnChange>(cancel);

        if (!_last_server_state || *_last_server_state < rs_on_change.new_state) {
            _last_server_state = rs_on_change.new_state;
            break;
        }
    }
    co_return;
}

net::awaitable<void> Client::run(Cancel cancel)
{
    using std::cerr;

    static constexpr bool dbg = false;

    while (true) {
        auto index = co_await fetch_index(cancel);

        _branch.merge_index(index);

        auto missing_blocks = _branch.missing_blocks();

        for (auto& block_id : missing_blocks) {
            if (index.object_is_missing(block_id)) {
                if (dbg) {
                    cerr << "C: Object " << block_id << " is missing at peer (skipping)\n";
                }
                continue;
            }

            if (dbg) {
                cerr << "C: Requesting: " << block_id << "\n";
            }

            auto block = co_await fetch_block(block_id, cancel);

            if (!block) {
                if (dbg) {
                    cerr << "C: Peer doesn't have object: " << block_id << "\n";
                }
                // Break the loop to download a new index.
                break;
            }

            if (dbg) {
                cerr << "C: Got: " << block_id << "\n";
            }

            _branch.store(*block);
        }

        co_await wait_for_a_change(cancel);

        if (dbg) {
            cerr << "C: restart\n";
        }
    }
}
