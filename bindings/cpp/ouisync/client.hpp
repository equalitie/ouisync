#pragma once

#include "ouisync/message.hpp"
#include <type_traits>
#include <boost/asio/async_result.hpp>
#include <boost/asio/experimental/channel.hpp>
#include <boost/asio/spawn.hpp> // yield_context
#include <boost/asio/any_completion_handler.hpp>
#include <boost/filesystem/path.hpp>
#include <boost/system/detail/error_code.hpp>

#include <ouisync/error.hpp>
#include <ouisync/handler.hpp>

namespace ouisync {

namespace detail {
    template<typename T> struct InvokeSig       { using type = void(boost::system::error_code, T); };
    template<>           struct InvokeSig<void> { using type = void(boost::system::error_code);    };

    using SubscriptionChannel = boost::asio::experimental::channel<void(boost::system::error_code, Response)>;

} // detail namespace

template<typename Variant>
class Subscription;

const std::size_t default_subscription_buffer_size = 32;

class Client : public std::enable_shared_from_this<Client> {
public:
    struct State;

    Client(Client&&) = default;
    Client(const Client&) = delete;
    ~Client();

    static Client connect(
        const boost::filesystem::path& config_dir_path,
        boost::asio::yield_context
    );

    /**
     * Send request and await response
     *
     * May throw with code from:
     *   * ouisync::error::client_error_category():
     *      if error originated in the client code
     *   * ouisync::error::service_error_category():
     *      if error originated in the service
     *   * boost::system::system_category():
     *      if error was thrown from the underlying TCP socket
     */
    template<
        class Variant,
        boost::asio::completion_token_for<typename detail::InvokeSig<typename Variant::type>::type>
            CompletionToken
    >
    auto invoke(Request request, CompletionToken token) {
        using RetType = Variant::type;
        auto self = shared_from_this();
        auto msg_id = next_message_id();

        return boost::asio::async_initiate<CompletionToken, typename detail::InvokeSig<RetType>::type>(
            [ msg_id,
              request = std::move(request),
              client = std::move(self)
            ](auto handler) {
                auto exec = client->get_executor();
                auto state = client->_state;

                Client::invoke_impl(
                    std::move(state),
                    msg_id,
                    std::move(request),
                    [ handler = std::move(handler),
                      client = std::move(client),
                      work_guard = boost::asio::make_work_guard(std::move(exec))
                    ] (boost::system::error_code ec, Response response) mutable {
                        if constexpr (std::is_void_v<RetType>) {
                            if (ec) {
                                handler(ec);
                            } else {
                                extract<Variant>(std::move(response), std::move(client));
                                handler(boost::system::error_code());
                            }
                        } else {
                            if (ec) {
                                handler(ec, RetType {});
                            } else {
                                handler(
                                    boost::system::error_code(),
                                    extract<Variant>(std::move(response), std::move(client))
                                );
                            }
                        }
                    });
            },
            token
        );
    }

    template<class Variant>
    Subscription<Variant> subscribe(Request request, std::size_t buffer_size = default_subscription_buffer_size) {
        auto channel = std::make_shared<detail::SubscriptionChannel>(get_executor(), buffer_size);
        auto msg_id = next_message_id();
        Subscription<Variant> sub(shared_from_this(), channel, msg_id);

        subscribe_impl(msg_id, std::move(request), std::move(channel));

        return sub;
    }

    bool is_connected() const noexcept;

    boost::asio::any_io_executor get_executor() const;

private:
    Client(std::shared_ptr<State>&&);

    MessageId next_message_id();

    static
    void receive_job(std::shared_ptr<State>, boost::asio::yield_context);

    static
    void invoke_impl(
        std::shared_ptr<State>,
        MessageId,
        Request,
        boost::asio::any_completion_handler<HandlerSig>
    );

    void subscribe_impl(MessageId, Request, std::shared_ptr<detail::SubscriptionChannel>);
    void unsubscribe_impl(MessageId);

private:
    std::shared_ptr<State> _state;

    template<typename> friend class Subscription;
};

/// Event subscription.
template<typename Variant>
class Subscription {
public:
    using Result = std::variant<boost::system::error_code, typename Variant::type>;

    explicit Subscription(
        std::shared_ptr<Client> client,
        std::shared_ptr<detail::SubscriptionChannel> channel,
        MessageId msg_id
    ): client(std::move(client)),
       channel(std::move(channel)),
       msg_id(msg_id)
    {   }

    Subscription(const Subscription<Variant>&) = delete;
    Subscription(Subscription<Variant>&&) = default;
    Subscription<Variant>& operator = (const Subscription<Variant>) = delete;
    Subscription<Variant>& operator = (Subscription<Variant>&&) = default;

    ~Subscription() {
        if (client) {
            client->unsubscribe_impl(msg_id);
        }
    }

    /// Receive next event from the subscription.
    template<boost::asio::completion_token_for<typename detail::InvokeSig<Variant>::type> CompletionToken>
    auto async_receive(CompletionToken token) {
        using Type = Variant::type;

        return boost::asio::async_initiate<
            CompletionToken,
            typename detail::InvokeSig<Type>::type
        >(
            [&](auto handler) {
                channel->async_receive(
                    [&, handler = std::move(handler)](boost::system::error_code ec, Response response) mutable {
                        if constexpr (std::is_void_v<Type>) {
                            if (ec) {
                                handler(ec);
                            } else {
                                extract<Variant>(std::move(response), client);
                                handler(boost::system::error_code());
                            }
                        } else {
                            if (ec) {
                                handler(ec, Type {});
                            } else {
                                handler(ec, extract<Variant>(std::move(response), client));
                            }
                        }
                    }
                );
            },
            token
        );
    }

    /// Try to receive the next event without blocking. Returns the next event (or error) if any is
    /// buffered, or std::nullopt otherwise.
    std::optional<Result> try_receive() {
        std::optional<Result> out = std::nullopt;

        channel->try_receive([&](boost::system::error_code ec, Response response) {
            if (ec) {
                out = std::optional { Result { ec } };
            } else {
                out = std::optional {
                    Result { extract<Variant>(std::move(response), client) }
                };
            }
        });

        return out;
    }

private:
    std::shared_ptr<Client> client;
    std::shared_ptr<detail::SubscriptionChannel> channel;
    MessageId msg_id;
};

} // namespace ouisync
