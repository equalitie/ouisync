#include "client.h"
#include "object/tree.h"
#include "object/blob.h"

#include <iostream>

using namespace ouisync;
using std::move;
using Branch = Repository::Branch;
using Tree = object::Tree;
using Blob = object::Blob;

Client::Client(MessageBroker::Client&& broker, Repository& repo) :
    _broker(move(broker)),
    _repo(repo)
{}

template<class T>
net::awaitable<T> Client::receive(Cancel cancel)
{
    auto rs = co_await _broker.receive(cancel);
    auto t = boost::get<T>(&rs);
    if (!t) throw_error(sys::errc::protocol_error);
    co_return std::move(*t);
}

net::awaitable<void>
Client::download_branch(RemoteBranch& branch, Id object_id, Cancel cancel)
{
    using AwaitVoid = net::awaitable<void>;

    co_await _broker.send(RqObject{object_id}, cancel);
    auto rs = co_await receive<RsObject>(cancel);

    apply(rs.object,
        [&] (const Tree& tree) -> AwaitVoid {
            auto id = co_await branch.insert_tree(tree);
            assert(id == object_id);

            for (auto& [child_name, child_id] : tree) {
                (void) child_name;
                download_branch(branch, child_id, cancel);
            }
        },
        [&] (const Blob& blob) -> AwaitVoid {
            co_await branch.insert_blob(blob);
        });

    co_await branch.mark_complete(object_id);
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

        if (!branch) {
            continue;
        }

        co_await download_branch(*branch, branch->root_object_id(), cancel);
    }
}
