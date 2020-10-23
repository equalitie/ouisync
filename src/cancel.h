#pragma once

#include "intrusive_list.h"
#include "shortcuts.h"
#include <boost/optional.hpp>

namespace ouisync {

class Cancel;

namespace CancelDetail {
    class ConnectionBase {
        private:
        friend class ouisync::Cancel;
        virtual void execute() = 0;
        intrusive::list_hook _hook;
    };
}

class Cancel : private CancelDetail::ConnectionBase {
private:
    using ConnectionBase = CancelDetail::ConnectionBase;

public:
    template<class OnCancel>
    class Connection : public ConnectionBase
    {
        public:

        Connection() = default;

        Connection(Connection&& other)
            : _on_cancel(std::move(other._on_cancel))
        {
            other._on_cancel = boost::none;
            other._hook.swap_nodes(this->_hook);
        }

        Connection& operator=(Connection&& other) {
            _on_cancel = std::move(other._on_cancel);
            other._on_cancel = boost::none;
            if (_hook.is_linked()) _hook.unlink();
            _hook.swap_nodes(other._hook);
            return *this;
        }

        private:

        friend class Cancel;

        void execute() override {
            if (!_on_cancel) return;
            auto on_cancel = std::move(*_on_cancel);
            _on_cancel = boost::none;
            on_cancel();
        }

        Opt<OnCancel> _on_cancel;
    };

public:
    Cancel() = default;

    Cancel(const Cancel&)            = delete;
    Cancel& operator=(const Cancel&) = delete;

    Cancel(Cancel& parent)
    {
        parent._connections.push_back(*this);
    }

    Cancel(Cancel&& other) :
        _connections(std::move(other._connections)),
        _call_count(other._call_count)
    {
        _hook.swap_nodes(other._hook);
        other._call_count = 0;
    }

    Cancel& operator=(Cancel&& other)
    {
        _connections = std::move(other._connections);

        if (_hook.is_linked()) _hook.unlink();
        _hook.swap_nodes(other._hook);

        _call_count = other._call_count;
        other._call_count = 0;

        return *this;
    }

    void operator()()
    {
        ++_call_count;
        auto cs = std::move(_connections);
        for (auto& c : cs) { c.execute(); }
    }

    size_t call_count() const { return _call_count; }

    operator bool() const { return call_count() != 0; }

    template<class OnCancel>
    [[nodiscard]]
    Connection<OnCancel> connect(OnCancel&& on_cancel)
    {
        Connection<OnCancel> connection;
        connection._on_cancel.emplace(std::forward<OnCancel>(on_cancel));
        _connections.push_back(connection);
        return connection;
    }

    size_t size() const { return _connections.size(); }

private:
    void execute() override { (*this)(); }

private:
    intrusive::list<ConnectionBase, &ConnectionBase::_hook> _connections;
    size_t _call_count = 0;
};

/*
 * Invokes cancelation from destructor
 */
class ScopedCancel : public Cancel {
public:
    using Cancel::Cancel;

    /*
     * Usecase:
     *
     * struct Foo {
     *     net::awaitable<void> Foo::async_fn(Cancel cancel) {
     *         // Invoke `cancel()` on scope exit
     *         auto cancel_con = _scoped_cancel.connect(cancel);
     *         ...
     *     }
     *
     *     ScopedCancel _scoped_cancel;
     * }
     */
    template<class OnCancel>
    [[nodiscard]]
    auto connect(Cancel& c)
    {
        return Cancel::connect([&c] { c(); });
    }

    /*
     * Usecase:
     *
     * struct Foo {
     *     net::awaitable<void> Foo::async_fn(Cancel cancel) {
     *         auto cancel_con = _scoped_cancel.connect(cancel, [&] { _timer.cancel(); });
     *
     *         // The above is a shorthand for:
     *         // auto cancel_con1 = cancel.connect([&] { _timer.cancel(); });
     *         // auto cancel_con2 = _scoped_cancel.connect([&] { cancel(); });
     *         ...
     *     }
     *
     *     ScopedCancel _scoped_cancel;
     *     net::deadline_timer _timer;
     * }
     */
    template<class OnCancel>
    [[nodiscard]]
    auto connect(Cancel& c, OnCancel&& on_cancel_)
    {
        auto con_ = c.connect([
            on_cancel = std::forward<OnCancel>(on_cancel_)
        ] {
            on_cancel();
        });

        return Cancel::connect([
            &c,
            con = std::move(con_)
        ] {
            c();
        });
    }

    ~ScopedCancel() {
        Cancel::operator()();
    }
};

} // namespace
