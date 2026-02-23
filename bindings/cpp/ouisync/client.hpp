#pragma once

#include <boost/asio/spawn.hpp> // yield_context
#include <boost/asio/any_completion_handler.hpp>
#include <boost/filesystem/path.hpp>
#include <ouisync/subscriber_id.hpp>
#include <ouisync/handler.hpp>

namespace ouisync {

class Client {
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
        boost::asio::completion_token_for<HandlerSig> CompletionToken
    >
    auto invoke(Request request, CompletionToken token) {
        return boost::asio::async_initiate<CompletionToken, HandlerSig>(
            [ state = _state,
              request = std::move(request),
              exec = get_executor()
            ](auto handler) {
                Client::invoke_impl(
                    std::move(state),
                    std::move(request),
                    [ handler = std::move(handler),
                      work_guard = boost::asio::make_work_guard(exec)
                    ]
                    (boost::system::error_code ec, Response response) mutable {
                        handler(ec, std::move(response));
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
