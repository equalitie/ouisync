#include <ouisync/subscription.hpp>
#include <ouisync/message.hpp>
#include <ouisync/api.g.hpp>
#include <ouisync/client.hpp>
#include <ouisync/error.hpp>
#include <ouisync/utils.hpp>

#include <boost/asio/any_completion_handler.hpp>
#include <iostream>

namespace ouisync {

namespace asio = boost::asio;
using Handler = asio::any_completion_handler<void(std::exception_ptr)>;

using State = std::variant<
    std::monostate,
    HandlerResult,
    Handler
>;

struct RepositorySubscription::Impl {
    asio::any_io_executor exec;
    std::shared_ptr<Client> client;
    SubscriberId subscriber_id;
    RepositoryHandle repository_handle;
    State state;

    void unsubscribe(asio::yield_context yield) {
        cancel();
        if (client->is_connected()) {
            client->unsubscribe(repository_handle, subscriber_id, yield);
        }
    }

    void cancel() {
        std::visit(overloaded {
            [&](std::monostate&) {},
            [&](HandlerResult&) {
                state = std::monostate();
            },
            [&](Handler& handler) {
                auto h = std::move(handler);
                state = std::monostate();

                asio::post(exec, [
                    h = std::move(h)
                ]() mutable {
                    try {
                        throw_error(asio::error::operation_aborted);
                    } catch (...) {
                        h(std::current_exception());
                    }
                });
            }
        },
        state);
    }
};

static bool is_error(const HandlerResult& result) {
    return std::visit(overloaded {
        [](const std::exception_ptr&) { return true; },
        [](const Response&) { return false; }
    }, result);
}

void RepositorySubscription::subscribe(Repository& repo, asio::yield_context yield) {
    if (_impl) {
        throw_error(error::already_subscribed);
    }

    auto client = repo.client;
    auto handle = repo.handle;

    auto subscriber_id = client->new_subscriber_id();

    _impl = std::make_shared<Impl>(Impl {
        yield.get_executor(),
        client,
        subscriber_id,
        handle,
        State{std::monostate()}
    });

    auto on_receive = [impl = _impl](HandlerResult result) {
        std::visit(overloaded {
            [&](std::monostate&) {
                impl->state.emplace<HandlerResult>(std::move(result));
            },
            [&](HandlerResult& old_result) {
                if (!is_error(old_result)) {
                    impl->state.emplace<HandlerResult>(std::move(result));
                }
            },
            [&](Handler& handler) {
                auto h = std::move(handler);
                impl->state = std::monostate();

                asio::post(impl->exec, [
                    h = std::move(h),
                    result = std::move(result)
                ]() mutable {
                    std::visit(overloaded {
                        [&](std::exception_ptr& eptr) { h(eptr); },
                        [&](const Response&) { h(nullptr); }
                    },
                    result);
                });
            }
        },
        impl->state);
    };

    try {
        client->subscribe(handle, subscriber_id, std::move(on_receive), yield);
    }
    catch (...) {
        _impl = nullptr;
    }
}

void RepositorySubscription::state_changed(boost::asio::yield_context yield) {
    if (!_impl) {
        throw_error(error::not_subscribed);
    }

    asio::async_initiate<decltype(yield), void(std::exception_ptr)>(
        [impl = _impl](auto handler) {
            std::visit(overloaded {
                [&](std::monostate) {
                    impl->state.emplace<Handler>(std::move(handler));
                },
                [&](HandlerResult& result_) {
                    auto result = std::move(result_);
                    impl->state = std::monostate();

                    std::visit(overloaded {
                        [&](std::exception_ptr eptr) {
                            handler(eptr);
                        },
                        [&](Response) {
                            handler(nullptr);
                        }
                    },
                    std::move(result));
                },
                [&](Handler&) {
                    try {
                        throw std::runtime_error("already awaiting");
                    } catch (...) {
                        handler(std::current_exception());
                    }
                }
            },
            impl->state);
        },
        yield
    );
}

void RepositorySubscription::unsubscribe(asio::yield_context yield) {
    if (!_impl) {
        return;
    }

    auto impl = std::move(_impl);
    impl->unsubscribe(yield);
}

RepositorySubscription::~RepositorySubscription() {
    if (!_impl) {
        return;
    }

    auto impl = std::move(_impl);

    boost::asio::spawn(impl->exec,
        [impl](auto yield) {
            impl->unsubscribe(yield);
        },
        [] (std::exception_ptr eptr) {
            try {
                if (eptr) {
                    std::rethrow_exception(eptr);
                }
            } catch (std::exception const& e) {
                std::cout << "Warning: failed to unsubscribe: " << e.what() << "\n";
            } catch (...) {
                std::cout << "Warning: failed to unsubscribe\n";
            }
        }
    );
}

} // namespace ouisync
