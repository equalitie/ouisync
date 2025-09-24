#include <ouisync/service.hpp>
#include <ouisync/data.hpp>
#include <ouisync/error.hpp>
#include <mutex>

#include <boost/asio/any_completion_handler.hpp>
#include <boost/asio/detached.hpp>

#include <iostream>

typedef void(Callback)(const void* context, ouisync::error::Service);

// Defined in the Rust code
// Returns opaque handle which needs to be passed to `stop_service`
extern "C" void* start_service(
    const char* config_dir,
    const char* debug_label,
    Callback,
    const void* context
);

// Defined in the Rust code
extern "C" void stop_service(
    const void* stop_handle,
    Callback,
    const void* context
);

namespace ouisync {

namespace asio = boost::asio;
namespace fs = boost::filesystem;
namespace system = boost::system;

using HandlerSig = void(system::error_code ec);
using Handler = asio::any_completion_handler<HandlerSig>;

struct Service::State {
    boost::asio::any_io_executor exec;
    void* stop_handle;

    State(asio::any_io_executor exec) :
        exec(std::move(exec)), stop_handle(nullptr) {}
};

Service::Service(boost::asio::any_io_executor exec) :
    _state(std::make_shared<State>(std::move(exec)))
{
}

struct StartContext {
    // To ensure that stop_handle has been set before the `handler` is
    // executed.
    std::shared_ptr<std::mutex> stop_handle_mutex;
    asio::executor_work_guard<asio::any_io_executor> work_guard;
    Handler handler;
};

static
void start_callback(const void* context, ouisync::error::Service ec) {
    auto ctx = std::unique_ptr<StartContext>((StartContext*)(context));

    // Second lock ensures that stop_handle has already been set
    ctx->stop_handle_mutex->lock();

    // This function might be called from another thread, so must `post` the
    // handler back.
    asio::post(ctx->work_guard.get_executor(), [
            handler = std::move(ctx->handler),
            ec
        ] () mutable {
            std::move(handler)(ec);
        }
    );
}

void Service::start(
    const fs::path& config_dir,
    const char* debug_label,
    asio::yield_context yield
) {
    auto started = asio::async_initiate<decltype(asio::deferred), HandlerSig>(
        [&](auto handler) {
            auto stop_handle_mutex = std::make_shared<std::mutex>();

            auto start_context = new StartContext {
                stop_handle_mutex,
                asio::make_work_guard(yield.get_executor()),
                std::move(handler),
            };

            // First lock
            stop_handle_mutex->lock();

            _state->stop_handle = start_service(
                config_dir.c_str(),
                debug_label,
                start_callback,
                start_context
            );

            // After unlocking the start_callback can proceed with calling the
            // handler.
            stop_handle_mutex->unlock();
        },
        asio::deferred
    );

    system::error_code* caller_ec = yield.ec_;
    system::error_code ec;

    started(yield[ec]);

    if (ec) {
        if (caller_ec) {
            *caller_ec = ec;
        } else {
            throw system::system_error(ec, "Failed to start Ouisync session");
        }
    }
}

struct StopContext {
    asio::executor_work_guard<asio::any_io_executor> work_guard;
    Handler handler;
};

static
void stop_callback(const void* context, ouisync::error::Service ec) {
    auto ctx = std::unique_ptr<StopContext>((StopContext*)(context));

    auto exec = ctx->work_guard.get_executor();

    // This function might be called from another thread, so must `post` the
    // handler back.
    asio::post(exec, [
            ctx = std::move(ctx),
            ec
        ] () mutable {
            std::move(ctx->handler)(ec);
        }
    );
}

void Service::stop(asio::yield_context yield) {
    if (!is_running()) {
        return;
    }

    auto stopped = asio::async_initiate<decltype(asio::deferred), HandlerSig>(
        [state = std::move(_state), &yield](auto handler) {
            auto stop_context = new StopContext {
                asio::make_work_guard(yield.get_executor()),
                std::move(handler),
            };

            stop_service(
                state->stop_handle,
                stop_callback,
                stop_context
            );
        },
        asio::deferred
    );

    system::error_code* caller_ec = yield.ec_;
    system::error_code ec;

    stopped(yield[ec]);

    if (ec) {
        if (caller_ec) {
            *caller_ec = ec;
        } else {
            throw system::system_error(ec, "Failed to stop Ouisync session");
        }
    }
}

Service::~Service() {
    if (!is_running()) {
        return;
    }

    auto state = _state;
    asio::spawn(state->exec,
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
    return _state && _state->stop_handle;
}

} // namespace ouisync
