#pragma once

#include <boost/asio/spawn.hpp> // yield_context
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
    Response invoke(const Request&, boost::asio::yield_context);

    ~Client();

    SubscriberId new_subscriber_id();
    void subscribe(const RepositoryHandle&, SubscriberId, std::function<HandlerSig>, boost::asio::yield_context);
    void unsubscribe(const RepositoryHandle&, SubscriberId, boost::asio::yield_context yield);

    bool is_connected() const noexcept;

private:
    Client(std::shared_ptr<State>&&);

    static
    void receive_job(std::shared_ptr<State>, boost::asio::yield_context);

private:
    std::shared_ptr<State> _state;
};

} // namespace ouisync
