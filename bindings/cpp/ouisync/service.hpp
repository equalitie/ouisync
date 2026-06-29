#pragma once

#include <boost/asio/any_completion_handler.hpp>
#include <boost/asio/async_result.hpp>
#include <boost/filesystem/path.hpp>
#include <ouisync/lib_api.hpp>

namespace ouisync {

class OUISYNC_SERVICE_API Service {
    using HandlerSig = void(boost::system::error_code);
    using Handler = boost::asio::any_completion_handler<HandlerSig>;

public:
    Service(boost::asio::any_io_executor exec);

    Service(Service&&);
    Service& operator = (Service&&);

    Service(const Service&) = delete;
    Service& operator=(const Service&) = delete;

    ~Service();

    /**
     * Starts ouisync service
     *
     * @param config_dir Path to a directory where the service will store
     *                   config files and repository databases.
     *
     * @param debug_label A label shown in debug output lines. Must be zero
     *                    terminated C string or `nullptr`.
     */
    template<typename CompletionToken>
    requires boost::asio::completion_token_for<
        CompletionToken,
        void(boost::system::error_code)
    >
    auto start(
        const boost::filesystem::path& config_dir,
        const char* debug_label,
        CompletionToken token
    ) {
        return boost::asio::async_initiate<CompletionToken, HandlerSig>(
            [&](auto handler) { start_impl(config_dir, debug_label, std::move(handler)); },
            token
        );
    }

    template<typename CompletionToken>
    requires boost::asio::completion_token_for<
        CompletionToken,
        void(boost::system::error_code)
    >
    auto stop(CompletionToken token) {
        return boost::asio::async_initiate<CompletionToken, HandlerSig>(
            [&](auto handler) { stop_impl(std::move(handler)); },
            token
        );
    }

    bool is_running() const;

    const boost::asio::any_io_executor& get_executor();

private:

    void start_impl(
        const boost::filesystem::path& config_dir,
        const char* debug_label,
        Handler
    );

    void stop_impl(Handler);

private:
    boost::asio::any_io_executor _exec;
    void* _stop_handle;

    friend struct StartContext;
    friend struct StopContext;
};

OUISYNC_SERVICE_API void init_log();

} // namespace ouisync
