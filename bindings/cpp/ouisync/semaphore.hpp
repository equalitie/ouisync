#pragma once

#include <boost/asio/any_io_executor.hpp>
#include <boost/asio/any_completion_handler.hpp>
#include <boost/asio/executor_work_guard.hpp>
#include <memory>
#include <queue>

namespace ouisync {

// Asynchronously ensure at most one access to a resource.
class Semaphore {
public:
    class Permit; // forward declare
private:
    using GetPermitSig = void(boost::system::error_code, Permit);

    struct Waiter {
        boost::asio::executor_work_guard<boost::asio::any_io_executor> work_guard;
        boost::asio::any_completion_handler<GetPermitSig> handler;
    };

    struct State : std::enable_shared_from_this<State> {
        bool semaphore_destroyed = false;
        bool has_active_permit = false;
        std::queue<Waiter> wait_queue;

        void on_permit_destroyed() {
            if (wait_queue.empty()) {
                has_active_permit = false;
                return;
            }

            auto waiter = std::move(wait_queue.front());
            wait_queue.pop();

            serve_waiter(std::move(waiter));
        }

        void on_semaphore_destroyed() {
            semaphore_destroyed = true;

            for (; !wait_queue.empty(); wait_queue.pop()) {
                serve_waiter(std::move(wait_queue.front()));
            }
        }

        void serve_waiter(Waiter&& waiter) {
            auto exec = waiter.work_guard.get_executor();

            boost::asio::post(exec,
                [ handler = std::move(waiter.handler),
                  state = shared_from_this()
                ] () mutable {
                    boost::system::error_code ec;
                    if (state->semaphore_destroyed) {
                        ec = boost::asio::error::operation_aborted;
                    }
                    handler(ec, Permit(std::move(state)));
                });
        }
    };

public:
    class Permit {
    private:
        std::shared_ptr<State> state;

    public:
        Permit(std::shared_ptr<State> state)
            : state(std::move(state))
        {}

        Permit(Permit const&) = delete;
        Permit(Permit&&) = default;

        ~Permit() {
            if (!state) return; // moved
            state->on_permit_destroyed();
        }
    };

    Semaphore(boost::asio::any_io_executor executor)
        : _executor(std::move(executor)),
          _state(std::make_shared<State>())
    {}

    template<
        boost::asio::completion_token_for<GetPermitSig> CompletionToken
    >
    auto get_permit(CompletionToken&& token) {
        return boost::asio::async_initiate<CompletionToken, GetPermitSig>(
            [ state = _state,
              exec = _executor
            ](auto handler) {
                if (!state->has_active_permit) {
                    state->has_active_permit = true;
                    handler(boost::system::error_code(), Permit(state));
                    return;
                }

                state->wait_queue.emplace(
                    boost::asio::make_work_guard(exec),
                    std::move(handler)
                );
            },
            token
        );
    }

    ~Semaphore() {
        if (!_state) {
            // Moved from
            return;
        }
        _state->on_semaphore_destroyed();
    }

private:
    boost::asio::any_io_executor _executor;
    std::shared_ptr<State> _state;
};

} // namespace
