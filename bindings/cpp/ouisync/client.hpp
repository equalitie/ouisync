#pragma once

#include <boost/asio/spawn.hpp> // yield_context
#include <boost/asio/any_completion_handler.hpp>
#include <boost/filesystem/path.hpp>
#include <ouisync/subscriber_id.hpp>
#include <ouisync/handler.hpp>

namespace ouisync {

class File;
class Repository;

namespace detail {
    template<typename T> struct InvokeSig       { using type = void(boost::system::error_code, T); };
    template<>           struct InvokeSig<void> { using type = void(boost::system::error_code);    };

    template<typename T> struct IsApiClass             : std::false_type {};
    template<>           struct IsApiClass<File>       : std::true_type  {};
    template<>           struct IsApiClass<Repository> : std::true_type  {};

    template<typename T> concept ApiClass = IsApiClass<T>::value;

    /*
     * Converts from `Response` variant to `RetType` and applies it to the handler.
     */
    template<typename Variant, typename RetType>
    struct ConvertResponse {
        template<class Handler>
        static void apply(Handler&& handler, boost::system::error_code ec, Response&& response) {
            if (ec) {
                handler(ec, RetType{});
                return;
            }
            Variant* rsp = response.template get_if<Variant>();
            if (rsp == nullptr) {
                handler(error::protocol, RetType{});
                return;
            }
            handler(ec, std::move(rsp->value));
        }
    };

    template<>
    struct ConvertResponse<Response::None, void> {
        template<class Handler>
        static void apply(Handler&& handler, boost::system::error_code ec, Response&& response) {
            if (ec) {
                handler(ec);
                return;
            }
            if (response.template get_if<Response::None>() == nullptr) {
                ec = error::protocol;
            }
            handler(ec);
        }
    };

    template<typename Variant, typename RetType>
    struct ConvertResponse<Variant, std::optional<RetType>> {
        template<class Handler>
        static void apply(Handler&& handler, boost::system::error_code ec, Response&& response) {
            if (ec) {
                handler(ec, RetType{});
                return;
            }
            if (response.template get_if<Response::None>() == nullptr) {
                handler(ec, RetType{});
                return;
            }
            auto rsp = response.template get_if<Variant>();
            if (rsp == nullptr) {
                handler(error::protocol, RetType{});
                return;
            }
            handler(ec, std::move(rsp->value));
        }
    };

    template<
        typename Variant,
        ApiClass RetType
    >
    struct ConvertResponse<Variant, RetType> {
        template<class ClientPtr, class Handler>
        static void apply(Handler&& handler, boost::system::error_code ec, Response&& response, ClientPtr client) {
            if (ec) {
                handler(ec, {});
                return;
            }
            Variant* rsp = response.template get_if<Variant>();
            if (rsp == nullptr) {
                handler(error::protocol, {});
                return;
            }
            handler(ec, RetType(std::move(client), std::move(rsp->value)));
        }
    };
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
        class Variant, // expected `Response` variant type
        class RetType, // return type
        boost::asio::completion_token_for<typename detail::InvokeSig<RetType>::type> CompletionToken
    >
    auto invoke(Request request, CompletionToken token) {
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
                        if constexpr (detail::IsApiClass<RetType>::value) {
                            detail::ConvertResponse<Variant, RetType>::apply(std::move(handler), ec, std::move(response), client);
                        }
                        else {
                            detail::ConvertResponse<Variant, RetType>::apply(std::move(handler), ec, std::move(response));
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
