#pragma once

#include "cancel.h"
#include <boost/optional.hpp>
#include <boost/asio/use_awaitable.hpp>
#include <boost/asio/io_context.hpp>

namespace ouisync {

/*
 * Waits for all members of a set of coroutines to finish a task.
 *
 * Usage:
 *
 * Barrier barrier(ioc);
 *
 * co_spawn(ioc, [lock = barrier.lock()]() -> net::awaitable<void> {
 *     co_await do_something();
 * });
 *
 * co_spawn(ioc, [lock = barrier.lock()]() -> net::awaitable<void> {
 *     co_await do_something();
 *     // This is unnecessary, for release() is called on lock destructor
 *     lock.release();
 * })
 *
 * // Returns when both of the above coroutines have called release()
 * // or the destructor on their lock
 * co_await barrier.wait();
 */

class Barrier {
public:
    using executor_type = net::io_context::executor_type;

public:
    class Lock {
        friend class Barrier;

        private:
        Lock() = default;

        public:
        Lock(const Lock&)            = delete;
        Lock& operator=(const Lock&) = delete;

        Lock(Lock&& other) : barrier(other.barrier)
        {
            hook.swap_nodes(other.hook);
            other.barrier = nullptr;
        }

        Lock& operator=(Lock&& other) {
            barrier = other.barrier;
            other.barrier = nullptr;
            if (hook.is_linked()) hook.unlink();
            hook.swap_nodes(other.hook);
            return *this;
        }

        ~Lock();
        void release();

        private:
        intrusive::list_hook hook;
        Barrier* barrier;
    };

public:
    Barrier(net::io_context&);
    Barrier(executor_type);

    Barrier(const Barrier&) = delete;

    Barrier& operator=(Barrier&&)      = delete; // TODO
    Barrier& operator=(const Barrier&) = delete; // TODO

    [[nodiscard]] Lock lock();
    [[nodiscard]] net::awaitable<void> wait(Cancel cancel = Cancel());

private:
    using Sig = void(sys::error_code);
    using AsyncResult = net::async_result<std::decay_t<decltype(net::use_awaitable)>, Sig>;
    using Handler = AsyncResult::handler_type;

    struct WaitEntry {
        Cancel* cancel = nullptr;
        intrusive::list_hook hook;
        Opt<Handler> handler;
    };

private:
    void try_release_all();
    void release_single(WaitEntry&);

private:
    executor_type _ex;
    intrusive::list<Lock, &Lock::hook> _locks;
    intrusive::list<WaitEntry, &WaitEntry::hook> _wait_entries;
};

inline Barrier::Barrier(net::io_context& ioc) :
    _ex(ioc.get_executor())
{}

inline Barrier::Barrier(executor_type exec) :
    _ex(std::move(exec))
{}

inline Barrier::Lock Barrier::lock()
{
    Lock lock;
    lock.barrier = this;
    _locks.push_back(lock);
    return lock;
}

inline void Barrier::release_single(WaitEntry& wait_entry)
{
    assert(wait_entry.handler && wait_entry.cancel);

    if (!wait_entry.handler) return;

    net::post(_ex, [
        h = std::move(*wait_entry.handler),
        c = wait_entry.cancel
    ] () mutable {
        sys::error_code ec;
        if (c && *c) ec = net::error::operation_aborted;
        h(ec);
    });
}

inline void Barrier::try_release_all()
{
    if (!_locks.empty()) return;
    auto es = std::move(_wait_entries);
    for (auto& e : es) {
        release_single(e);
    }
}

inline void Barrier::Lock::release()
{
    if (!hook.is_linked()) return;
    hook.unlink();
    barrier->try_release_all();
}

inline Barrier::Lock::~Lock()
{
    release();
}

inline net::awaitable<void> Barrier::wait(Cancel cancel)
{
    if (_locks.empty()) co_return;

    WaitEntry wait_entry;
    wait_entry.cancel = &cancel;
    _wait_entries.push_back(wait_entry);

    auto cc = cancel.connect([&] {
        release_single(wait_entry);
    });

    // http://open-std.org/JTC1/SC22/WG21/docs/papers/2019/p1943r0.html
    co_await net::async_initiate<decltype(net::use_awaitable), Sig>(
        [&wait_entry] (auto&& h) {
            wait_entry.handler.emplace(std::move(h));
        },
        net::use_awaitable);
}

} // namespace
