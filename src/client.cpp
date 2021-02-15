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

net::awaitable<Branch::Indices> Client::fetch_indices(Cancel cancel)
{
    co_await _broker.send(RqIndices{}, cancel);
    auto rs_indices = co_await receive<RsIndices>(cancel);
    co_return move(rs_indices.indices);
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
    while (true) {
        auto indices = co_await fetch_indices(cancel);

        std::cerr << "Received indices\n";
        _branch.merge_indices(indices);

        std::cerr << "Missing objects: " << _branch.missing_objects() << "\n";
        for (auto& obj_id : _branch.missing_objects()) {
            std::cerr << "Requesting: " << obj_id << "\n";
            auto obj = co_await fetch_object(obj_id, cancel);

            if (!obj) {
                std::cerr << "Peer doesn't have object: " << obj_id << "\n";
                continue;
            }

            std::cerr << "Got: " << obj_id << "\n";

            apply(*obj,
                [&] (const FileBlob& file) {
                    _branch.objstore().store(file);
                },
                [&] (const Directory& dir) {
                    _branch.objstore().store(dir);
                });
        }

        co_await wait_for_a_change(cancel);
    }
}
