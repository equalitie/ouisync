#pragma once

#include <boost/system/detail/error_code.hpp>
#include <type_traits>
#include <boost/asio/spawn.hpp> // yield_context
#include <boost/asio/any_completion_handler.hpp>
#include <boost/filesystem/path.hpp>

#include <ouisync/error.hpp>
#include <ouisync/handler.hpp>
#include <ouisync/subscriber_id.hpp>

namespace ouisync {

class File;
class NetworkSocket;
class Repository;

namespace detail {
    template<typename T> struct InvokeSig       { using type = void(boost::system::error_code, T); };
    template<>           struct InvokeSig<void> { using type = void(boost::system::error_code);    };
} // detail namespace

class Client : public std::enable_shared_from_this<Client> {
public:
    struct State;

    Client(Client&&) = default;
    Client(const Client&) = delete;

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
        boost::asio::completion_token_for<
            typename detail::InvokeSig<typename Variant::type>::type
        > CompletionToken
    >
    auto invoke(Request request, CompletionToken token) {
        using RetType = Variant::type;
        auto self = shared_from_this();

        return boost::asio::async_initiate<CompletionToken, typename detail::InvokeSig<RetType>::type>(
            [ request = std::move(request),
              client = std::move(self)
            ](auto handler) {
                auto exec = client->get_executor();
                auto state = client->_state;

                Client::invoke_impl(
                    std::move(state),
                    std::move(request),
                    [ handler = std::move(handler),
                      client = std::move(client),
                      work_guard = boost::asio::make_work_guard(std::move(exec))
                    ]
                    (boost::system::error_code ec, Response response) mutable {
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

    ~Client();

    SubscriberId new_subscriber_id();

    void subscribe(const RepositoryHandle&, SubscriberId, std::function<InnerHandlerSig>, boost::asio::yield_context);
    void unsubscribe(const RepositoryHandle&, SubscriberId, boost::asio::yield_context);

    bool is_connected() const noexcept;

    boost::asio::any_io_executor get_executor() const;

private:
    Client(std::shared_ptr<State>&&);

    static
    void receive_job(std::shared_ptr<State>, boost::asio::yield_context);

    static
    void invoke_impl(std::shared_ptr<State>, Request, boost::asio::any_completion_handler<HandlerSig>);

private:
    std::shared_ptr<State> _state;
};

} // namespace ouisync
