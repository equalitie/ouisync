#pragma once

#include "namespaces.h"
#include "intrusive_list.h"
#include <boost/optional.hpp>
#include <boost/asio/use_awaitable.hpp>
#include <boost/asio/io_context.hpp>

namespace ouisync {

/*
 * Waits for all members of a set of coroutines to finish a task.
 *
 * Usage:
 *
 * Block block(ioc);
 *
 * co_spawn(ioc, [lock = block.lock()]() -> net::awaitable<void> {
 *     co_await do_something();
 * });
 *
 * co_spawn(ioc, [lock = block.lock()]() -> net::awaitable<void> {
 *     co_await do_something();
 *     // This is unnecessary, for release() is called on lock destructor
 *     lock.release();
 * })
 *
 * // Returns when both of the above coroutines have called release()
 * // or the destructor on their lock
 * co_await block.wait();
 */

class Block {
public:
    using executor_type = net::io_context::executor_type;

public:
    class Lock {
        friend class Block;

        private:
        Lock() = default;

        public:
        Lock(const Lock&)            = delete;
        Lock& operator=(const Lock&) = delete;

        Lock(Lock&& other) : block(other.block)
        {
            hook.swap_nodes(other.hook);
            other.block = nullptr;
        }

        Lock& operator=(Lock&& other) {
            block = other.block;
            other.block = nullptr;
            if (hook.is_linked()) hook.unlink();
            hook.swap_nodes(other.hook);
            return *this;
        }

        ~Lock();
        void release();

        private:
        intrusive::list_hook hook;
        Block* block;
    };

public:
    Block(net::io_context&);
    Block(executor_type);

    Block(const Block&) = delete;

    Block& operator=(Block&&)      = delete; // TODO
    Block& operator=(const Block&) = delete; // TODO

    [[nodiscard]] Lock lock();
    [[nodiscard]] net::awaitable<void> wait();

private:
    void try_unblock();

private:
    using Sig = void();
    using AsyncResult = net::async_result<std::decay_t<decltype(net::use_awaitable)>, Sig>;
    using Handler = AsyncResult::handler_type;

    struct WaitEntry {
        intrusive::list_hook hook;
        Opt<Handler> handler;
    };

private:
    executor_type _ex;
    intrusive::list<Lock, &Lock::hook> _locks;
    intrusive::list<WaitEntry, &WaitEntry::hook> _wait_entries;
};

inline Block::Block(net::io_context& ioc) :
    _ex(ioc.get_executor())
{}

inline Block::Block(executor_type exec) :
    _ex(std::move(exec))
{}

inline Block::Lock Block::lock()
{
    Lock lock;
    lock.block = this;
    _locks.push_back(lock);
    return lock;
}

inline void Block::try_unblock()
{
    if (!_locks.empty()) return;

    auto es = std::move(_wait_entries);

    for (auto& e : es) {
        assert(e.handler);
        if (!e.handler) continue;
        net::post(_ex, std::move(*e.handler));
    }
}

inline void Block::Lock::release()
{
    if (!hook.is_linked()) return;
    hook.unlink();
    block->try_unblock();
}

inline Block::Lock::~Lock()
{
    release();
}

inline net::awaitable<void> Block::wait()
{
    if (_locks.empty()) co_return;

    WaitEntry wait_entry;
    _wait_entries.push_back(wait_entry);

    // http://open-std.org/JTC1/SC22/WG21/docs/papers/2019/p1943r0.html
    co_await net::async_initiate<decltype(net::use_awaitable), Sig>(
        [&wait_entry] (auto&& h) {
            wait_entry.handler.emplace(std::move(h));
        },
        net::use_awaitable);
}

} // namespace
