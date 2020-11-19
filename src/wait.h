#pragma once

#include "cancel.h"
#include <boost/optional.hpp>
#include <boost/asio/use_awaitable.hpp>
#include <boost/asio/io_context.hpp>

namespace ouisync {

/*
 * Waits for something to happen.
 *
 * Usage:
 *
 * unsigned k = 2;
 * Wait on_coroutines_finish(ioc);
 *
 * co_spawn(ioc, [&]() -> net::awaitable<void> {
 *     auto on_exit = defer([&] { if (--k == 0) on_coroutines_finish.notify(); });
 *     co_await do_something_1();
 * });
 *
 * co_spawn(ioc, [&]() -> net::awaitable<void> {
 *     auto on_exit = defer([&] { if (--k == 0) on_coroutines_finish.notify(); });
 *     co_await do_something_2();
 * })
 *
 * // Unblocks once on_coroutines_finish.notify() is called.
 * co_await on_coroutines_finish.wait();
 */

class Wait {
public:
    using executor_type = net::any_io_executor;

public:
    Wait(net::io_context&);
    Wait(executor_type);

    Wait(const Wait&) = delete;
    Wait& operator=(const Wait&) = delete;

    Wait(Wait&& other) :
        _ex(other._ex),
        _wait_entries(std::move(other._wait_entries))
    {}

    Wait& operator=(Wait&&) = delete; // TODO

    void notify();

    [[nodiscard]] net::awaitable<void> wait(Cancel);

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
    void release_single(WaitEntry&);

private:
    executor_type _ex;
    intrusive::list<WaitEntry, &WaitEntry::hook> _wait_entries;
};

inline Wait::Wait(net::io_context& ioc) :
    _ex(ioc.get_executor())
{}

inline Wait::Wait(executor_type exec) :
    _ex(std::move(exec))
{}

inline void Wait::release_single(WaitEntry& wait_entry)
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

inline void Wait::notify()
{
    auto es = std::move(_wait_entries);
    for (auto& e : es) {
        release_single(e);
    }
}

inline net::awaitable<void> Wait::wait(Cancel cancel)
{
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
