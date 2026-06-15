#pragma once

#include <boost/asio/spawn.hpp>
#include <boost/filesystem/path.hpp>
#include <ouisync/lib_api.hpp>

namespace ouisync {

class OUISYNC_SERVICE_API Service {
public:
    Service(boost::asio::any_io_executor exec);

    /**
     * Starts ouisync service
     *
     * @param config_dir Path to a directory where the service will store
     *                   config files and repository databases.
     *
     * @param debug_label A label shown in debug output lines. Must be zero
     *                    terminated C string or `nullptr`.
     */
    void start(
        const boost::filesystem::path& config_dir,
        const char* debug_label,
        boost::asio::yield_context
    );

    void stop(boost::asio::yield_context);

    bool is_running() const;

    Service(Service&&) = default;

    Service(const Service&) = delete;
    Service& operator=(const Service&) = delete;

    ~Service();

private:
    struct State;
    std::shared_ptr<State> _state;
};

OUISYNC_SERVICE_API void init_log();

} // namespace ouisync
