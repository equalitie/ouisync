#include <iostream>

#include <boost/asio/detached.hpp>
#include <boost/asio/spawn.hpp>

#include <ouisync/service.hpp>
#include <ouisync/data.hpp>
#include <ouisync/error.hpp>

typedef void(Callback)(void* context, ouisync::error::Service);

// Defined in the Rust code
// Returns opaque handle which needs to be passed to `stop_service`
extern "C" void* start_service(
    const char* config_dir,
    const char* debug_label,
    Callback,
    void* context
);

// Defined in the Rust code
extern "C" void stop_service(
    const void* stop_handle,
    Callback,
    void* context
);

// Defined in Rust code
extern "C" void init_log();

namespace ouisync {

namespace asio = boost::asio;
namespace fs = boost::filesystem;

Service::Service(boost::asio::any_io_executor exec) :
    _exec(std::move(exec)),
    _stop_handle(nullptr)
{
}

Service::Service(Service&& other) :
    _exec(std::move(other._exec)),
    _stop_handle(other._stop_handle)
{
    other._stop_handle = nullptr;
}

Service& Service::operator = (Service&& other) {
    _exec = std::move(other._exec);
    _stop_handle = other._stop_handle;
    other._stop_handle = nullptr;

    return *this;
}

const asio::any_io_executor& Service::get_executor() {
    return _exec;
}

struct StartContext {
    // To ensure that stop_handle has been set before the `handler` is
    // executed.
    std::shared_ptr<std::mutex> stop_handle_mutex;
    asio::executor_work_guard<asio::any_io_executor> work_guard;
    Service::Handler handler;
};

static
void start_callback(void* context, ouisync::error::Service ec) {
    auto ctx = std::unique_ptr<StartContext>(static_cast<StartContext*>(context));

    // Second lock ensures that stop_handle has already been set
    ctx->stop_handle_mutex->lock();

    // This function might be called from another thread, so must `post` the
    // handler back.
    asio::post(
        ctx->work_guard.get_executor(),
        [handler = std::move(ctx->handler), ec] () mutable {
            std::move(handler)(ec);
        }
    );
}

#if defined(WIN32) || defined(_WIN32) || defined(__WIN32__) || defined(__MINGW32__) || defined(__CYGWIN__)
#  define OUISYNC_WINDOWS
#endif

#ifdef OUISYNC_WINDOWS
static std::string convert_wstring_to_string(std::wstring const& src) {
    size_t len = std::wcstombs(nullptr, src.c_str(), 0) + 1;
    std::string dst(len, '\0');
    std::wcstombs(&dst[0], src.c_str(), len);
    return dst;
}
#endif

void Service::start_impl(
    const fs::path& config_dir,
    const char* debug_label,
    Handler handler
) {
    if (is_running()) {
        handler(boost::system::error_code());
        return;
    }

    auto stop_handle_mutex = std::make_shared<std::mutex>();
    auto start_context = std::make_unique<StartContext>(
        stop_handle_mutex,
        asio::make_work_guard(_exec),
        std::move(handler)
    );

    // First lock
    stop_handle_mutex->lock();

    _stop_handle = start_service(
        #if !defined(OUISYNC_WINDOWS)
            config_dir.c_str(),
        #else
            convert_wstring_to_string(config_dir.native()).c_str(),
        #endif
        debug_label,
        start_callback,
        start_context.release()
    );

    // After unlocking the start_callback can proceed with calling the
    // handler.
    stop_handle_mutex->unlock();
}

struct StopContext {
    asio::executor_work_guard<asio::any_io_executor> work_guard;
    Service::Handler handler;
};

static
void stop_callback(void* context, ouisync::error::Service ec) {
    auto ctx = std::unique_ptr<StopContext>(static_cast<StopContext*>(context));

    auto exec = ctx->work_guard.get_executor();

    // This function might be called from another thread, so must `post` the
    // handler back.
    asio::post(
        exec,
        [ctx = std::move(ctx), ec] () mutable {
            std::move(ctx->handler)(ec);
        }
    );
}

void Service::stop_impl(Handler handler) {
    if (!is_running()) {
        handler(boost::system::error_code());
        return;
    }

    auto stop_handle = _stop_handle;
    _stop_handle = nullptr;

    auto stop_context = std::make_unique<StopContext>(
        asio::make_work_guard(_exec),
        std::move(handler)
    );

    stop_service(
        stop_handle,
        stop_callback,
        stop_context.release()
    );
}

Service::~Service() {
    if (!is_running()) {
        return;
    }

    auto exec = _exec;

    asio::spawn(
        exec,
        [self = std::move(*this)] (asio::yield_context yield) mutable {
            std::cout << "Stopping Ouisync service in detached mode...\n";
            try {
                self.stop(yield);
                std::cout << "Ouisync service stopped\n";
            }
            catch (const std::exception& e) {
                std::cout << "Ouisync service failed to stop: " << e.what() << "\n";
            }
        },
        asio::detached
    );
}

bool Service::is_running() const {
    return _stop_handle;
}

void init_log() {
    ::init_log();
}

} // namespace ouisync
