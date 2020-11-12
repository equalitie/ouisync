#pragma once

#include "cancel.h"
#include <boost/optional.hpp>
#include <boost/asio/use_awaitable.hpp>
#include <boost/asio/io_context.hpp>

namespace ouisync {

/*
 * Use Bouncer to allow only a single coroutine to access a resource
 *
 * Usage:
 *
 * Barrier barrier(ioc)
 * Bouncer bouncer(ioc);
 *
 * // We allow only a single coroutine to call `async_send` on the given socket.
 * tcp::socket socket;
 *
 * co_spawn(ioc, [&, lock = barrier.lock()]() -> net::awaitable<void> {
 *     auto socket_send_access = co_await bouncer.wait();
 *     co_await socket.async_send(...);
 * });
 *
 * co_spawn(ioc, [&, lock = barrier.lock()]() -> net::awaitable<void> {
 *     auto socket_send_access = co_await bouncer.wait();
 *     co_await socket.async_send(...);
 * })
 *
 * co_await barrier.wait();
 */

class Bouncer {
public:
    using executor_type = net::any_io_executor;

public:
    class Lock {
        friend class Bouncer;

        private:
        Lock() = default;

        public:
        Lock(const Lock&)            = delete;
        Lock& operator=(const Lock&) = delete;

        Lock(Lock&& other) : bouncer(other.bouncer)
        {
            hook.swap_nodes(other.hook);
            other.bouncer = nullptr;
        }

        Lock& operator=(Lock&& other) {
            bouncer = other.bouncer;
            other.bouncer = nullptr;
            if (hook.is_linked()) hook.unlink();
            hook.swap_nodes(other.hook);
            return *this;
        }

        ~Lock();
        void release();

        private:
        intrusive::list_hook hook;
        Bouncer* bouncer;
    };

public:
    Bouncer(net::io_context&);
    Bouncer(executor_type);

    Bouncer(const Bouncer&) = delete;
    Bouncer& operator=(const Bouncer&) = delete;

    Bouncer(Bouncer&& other) :
        _ex(other._ex),
        _locks(std::move(other._locks)),
        _wait_entries(std::move(other._wait_entries))
    {
        for (auto& l : _locks) l.bouncer = this;
    }

    Bouncer& operator=(Bouncer&&) = delete; // TODO

    [[nodiscard]] net::awaitable<Lock> wait(Cancel cancel);

private:
    using Sig = void(sys::error_code);
    using AsyncResult = net::async_result<std::decay_t<decltype(net::use_awaitable)>, Sig>;
    using Handler = AsyncResult::handler_type;

    struct WaitEntry {
        Cancel* cancel = nullptr;
        intrusive::list_hook hook;
        Opt<Handler> handler;
    };

    [[nodiscard]] Lock lock();

private:
    void try_release_first_in_queue();
    void release_single(WaitEntry&);

private:
    executor_type _ex;
    intrusive::list<Lock, &Lock::hook> _locks;
    intrusive::list<WaitEntry, &WaitEntry::hook> _wait_entries;
};

inline Bouncer::Bouncer(net::io_context& ioc) :
    _ex(ioc.get_executor())
{}

inline Bouncer::Bouncer(executor_type exec) :
    _ex(std::move(exec))
{}

inline Bouncer::Lock Bouncer::lock()
{
    Lock lock;
    lock.bouncer = this;
    _locks.push_back(lock);
    return lock;
}

inline void Bouncer::release_single(WaitEntry& wait_entry)
{
    assert(wait_entry.handler);
    assert(wait_entry.cancel);

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

inline void Bouncer::try_release_first_in_queue()
{
    // We currently let only one job at a time.
    assert(_locks.empty());
    if (!_locks.empty()) return;

    if (_wait_entries.empty()) return;
    release_single(_wait_entries.front());
    _wait_entries.pop_front();
}

inline void Bouncer::Lock::release()
{
    if (!hook.is_linked()) return;
    hook.unlink();
    bouncer->try_release_first_in_queue();
}

inline Bouncer::Lock::~Lock()
{
    release();
}

inline net::awaitable<Bouncer::Lock> Bouncer::wait(Cancel cancel)
{
    if (_locks.empty()) {
        co_return lock();
    }

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

    co_return lock();
}

} // namespace
