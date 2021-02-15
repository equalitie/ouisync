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

net::awaitable<void> Client::run(Cancel cancel)
{
    uint64_t state_counter = 0;

    while (true) {
        co_await _broker.send(RqIndices{}, cancel);
        auto rs_indices = co_await receive<RsIndices>(cancel);

        std::cerr << "Received indices\n";
        _branch.merge_indices(rs_indices.indices);

        std::cerr << "Missing objects: " << _branch.missing_objects() << "\n";
        for (auto& obj_id : _branch.missing_objects()) {
            std::cerr << "Requesting: " << obj_id << "\n";
            co_await _broker.send(RqObject{obj_id}, cancel);
            auto rs_obj = co_await receive<RsObject>(cancel);

            if (!rs_obj.object) {
                std::cerr << "Peer doesn't have object: " << obj_id << "\n";
                continue;
            }

            std::cerr << "Got: " << obj_id << "\n";

            apply(*rs_obj.object,
                [&] (const FileBlob& file) {
                    _branch.objstore().store(file);
                },
                [&] (const Directory& dir) {
                    _branch.objstore().store(dir);
                });
        }

        co_await _broker.send(RqNotifyOnChange{state_counter}, cancel);
        auto rs_on_change = co_await receive<RsNotifyOnChange>(cancel);
        state_counter = rs_on_change.new_state;
    }
}
