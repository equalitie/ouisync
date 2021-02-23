#include "client.h"
#include "error.h"
#include "directory.h"
#include "file_blob.h"

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

net::awaitable<Opt<RsObject::Object>> Client::fetch_object(const ObjectId& obj, Cancel cancel)
{
    co_await _broker.send(RqObject{obj}, cancel);
    auto rs_obj = co_await receive<RsObject>(cancel);
    co_return std::move(rs_obj.object);
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

    while (true) {
        auto index = co_await fetch_index(cancel);

        _branch.merge_index(index);

        auto missing_objects = _branch.missing_objects();

        for (auto& obj_id : missing_objects) {
            if (index.object_is_missing(obj_id)) {
                cerr << "C: Object " << obj_id << " is missing at peer (skipping)\n";
                continue;
            }

            cerr << "C: Requesting: " << obj_id << "\n";

            auto obj = co_await fetch_object(obj_id, cancel);

            if (!obj) {
                cerr << "C: Peer doesn't have object: " << obj_id << "\n";
                // Break the loop to download a new index.
                break;
            }

            cerr << "C: Got: " << obj_id << "\n";

            apply(*obj,
                [&] (const FileBlob& file) {
                    _branch.store(file);
                },
                [&] (const Directory& dir) {
                    _branch.store(dir);
                });
        }

        co_await wait_for_a_change(cancel);
    }
}
