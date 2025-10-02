#include <ouisync/client.hpp>
#include <ouisync/serialize.hpp>
#include <ouisync/debug.hpp> // debug
#include <ouisync/error.hpp>
#include <ouisync/utils.hpp>
#include <ouisync/subscriptions.hpp>

#include <boost/json.hpp>
#include <boost/hash2/sha2.hpp>
#include <boost/asio/ip/tcp.hpp>
#include <boost/asio/write.hpp>
#include <boost/asio/read.hpp>
#include <boost/json/src.hpp>

#include <ranges>
#include <fstream>
#include <random>

#include <unordered_map>

namespace ouisync {

namespace asio = boost::asio;
namespace system = boost::system;
namespace endian = boost::endian;

using Socket = asio::ip::tcp::socket;
// If this breaks due to being in the detail namespace, consider using
// asio::any_completion_handler<HandlerSig>
using ResponseHandler = asio::detail::spawn_handler<asio::any_io_executor, HandlerSig, void>;
using RawMessageId = decltype(MessageId::value);
using RawRepositoryHandle = decltype(RepositoryHandle::value);

// Marker to signify this pending entry correponds to a subscription
struct Subscription {};

using Pending = std::variant<
    // std::monostate is used when no handler has yet been set and no response has
    // arrived. This is needed because we get an async handler from
    // asio::async_initiate only once the operation starts to await, but we
    // need to "register" an entry beforehand in case that a response arrives
    // in the mean time.
    std::monostate,
    HandlerResult,
    ResponseHandler,
    Subscription
>;

struct Client::State : std::enable_shared_from_this<Client::State> {
    Socket socket;
    std::unordered_map<RawMessageId, Pending> pending;
    Subscriptions subscriptions;

    RawMessageId next_request_id = 0;
    SubscriberId next_subscriber_id = 0;

    State(Socket socket): socket(std::move(socket)) {}
};

static
void send(Socket& socket, MessageId rq_id, const Request& rq, asio::yield_context yield) {
    std::stringstream request_buffer = serialize(rq);

    uint64_t rq_id_be = endian::native_to_big(rq_id.value);

    uint32_t rq_size = sizeof(rq_id_be) + request_buffer.view().size();
    uint32_t rq_size_be = endian::native_to_big(rq_size);

    asio::async_write(
        socket,
        std::array<asio::const_buffer, 3>{
            asio::buffer(&rq_size_be, sizeof(rq_size_be)),
            asio::buffer(&rq_id_be, sizeof(rq_id_be)),
            asio::buffer(request_buffer.view())
        },
        yield);
}

HandlerResult to_handler_result(ResponseResult&& rs) {
    return std::visit(overloaded {
        [](ResponseResult::Failure&& failure) -> HandlerResult {
            return failure.move_to_exception_ptr();
        },
        [](Response&& response) -> HandlerResult {
            return std::move(response);
        }
    },
    std::move(rs.value));
}

static
std::tuple<MessageId, HandlerResult>
receive(Socket& socket, asio::yield_context yield) {
    uint32_t rs_size_be;
    asio::async_read(socket, asio::buffer(&rs_size_be, sizeof(rs_size_be)), yield);
    auto rs_size = endian::big_to_native(rs_size_be);

    uint64_t rs_id_be;

    if (rs_size < sizeof(rs_id_be)) {
        throw_error(error::protocol, "response too small");
    }

    std::vector<char> response_data(rs_size - sizeof(rs_id_be));
    asio::async_read(
        socket,
        std::array<asio::mutable_buffer, 2>{
            asio::buffer(&rs_id_be, sizeof(rs_id_be)),
            asio::buffer(response_data)
        },
        yield);

    ResponseResult response_result = deserialize(response_data);

    return {
        MessageId{endian::big_to_native(rs_id_be)},
        to_handler_result(std::move(response_result))
    };
}

/* static */
void Client::receive_job(std::shared_ptr<State> state, boost::asio::yield_context yield) {
    try {
        while (state->socket.is_open()) {
            auto [rs_id, rs] = receive(state->socket, yield);
            auto entry_i = state->pending.find(rs_id.value);
            if (entry_i == state->pending.end()) {
                continue;
            }
            auto& entry = entry_i->second;

            std::visit(overloaded {
                [&](std::monostate) {
                    entry.emplace<HandlerResult>(std::move(rs));
                },
                [&](const HandlerResult&) {
                    std::cout << "Warning: response received more than once (ignored)\n";
                },
                [&](ResponseHandler& handler) {
                    auto h = [
                        handler = std::move(handler),
                        rs = std::move(rs)
                    ] () mutable {
                        std::move(handler)(std::move(rs));
                    };
                    state->pending.erase(entry_i);
                    asio::post(yield.get_executor(), std::move(h));
                },
                [&](Subscription&) {
                    state->subscriptions.handle(yield.get_executor(), rs_id, rs);
                }
            },
            entry);
        }
    }
    catch (...) {
        std::exception_ptr eptr = std::current_exception();

        auto pending = std::move(state->pending);
        auto subscriptions = std::move(state->subscriptions);

        for (auto& [rq_id, entry] : pending) {
            std::visit(overloaded {
                [&](std::monostate) {
                    entry = eptr;
                },
                [&](const HandlerResult&) {
                    // Ignored, user will receive original response
                },
                [&](ResponseHandler& handler) {
                    auto h = [
                        handler = std::move(handler),
                        eptr
                    ] () mutable {
                        std::move(handler)(eptr);
                    };
                    asio::post(yield.get_executor(), std::move(h));
                },
                [&](Subscription&) {
                    // Subscriptions are handled below
                }
            },
            entry);
        }

        subscriptions.handle_all(yield.get_executor(), eptr);
    }
}

Client::Client(std::shared_ptr<State>&& state)
    : _state(state)
{
    // Start the receiving job
    asio::spawn(
        _state->socket.get_executor(),
        [state = _state](asio::yield_context yield) {
            receive_job(state, yield);
        },
        [](std::exception_ptr e) noexcept {
            // We're catching exceptions in the `receive_job`, so if we get
            // a non null `e` here, it's a bug and we log and terminate.
            try {
                if (e) std::rethrow_exception(e);
            }
            catch (const std::exception& e) {
                std::cout << "Uncaught exception: " << e.what() << "\n";
                std::terminate();
            }
            catch (...) {
                std::cout << "Uncaught exception: unknown\n";
                std::terminate();
            }
        }
    );
}

Client::~Client() {
    if (!_state) {
        // This client instance has been moved from
        return;
    }

    auto& socket = _state->socket;

    if (socket.is_open()) {
        socket.close();
    }
}

/**
 * Create an entry in `state->pending` with a handler that gets executed when a
 * response with `rq_id` arrives
 */
template<asio::completion_token_for<HandlerSig> CompletionToken>
auto add_pending(
    std::shared_ptr<Client::State> state,
    RawMessageId rq_id,
    CompletionToken&& token
) {
    state->pending.emplace(std::pair(rq_id, std::monostate()));

    return asio::async_initiate<CompletionToken, HandlerSig>(
        // Note that this lambda is executed only once the token is awaited.
        [state = std::move(state), rq_id](auto handler) {
            auto entry_i = state->pending.find(rq_id);
            auto& entry = entry_i->second;
            std::visit(overloaded {
                [&](std::monostate) {
                    entry.emplace<ResponseHandler>(std::move(handler));
                },
                [&](HandlerResult& rs) {
                    auto rs_ = std::move(rs);
                    state->pending.erase(entry_i);
                    std::move(handler)(std::move(rs_));
                },
                [&](ResponseHandler&) {
                    throw_error(error::logic, "Handler already set");
                },
                [&](Subscription&) {
                    throw_error(error::logic, "Response/subscription mismatch");
                },
            },
            entry);
        },
        token);
}

Response Client::invoke(const Request& request, asio::yield_context yield) {
    auto work_guard = asio::make_work_guard(yield.get_executor());
    auto rq_id = _state->next_request_id++;

    auto responded = add_pending(_state, rq_id, asio::deferred);
    send(_state->socket, MessageId{rq_id}, request, yield);

    HandlerResult handler_result;

    try {
        handler_result = responded(yield);
    } catch (...) {
        // Likely this is boost::context::detail::forced_unwind which could
        // happen if the handler was destroyed before it's executed. If unsure,
        // use this
        // https://stackoverflow.com/a/47164539/273348
        throw_error(error::logic, "unknown exception");
    }

    return std::visit(overloaded {
            [](std::exception_ptr eptr) -> Response {
                if (eptr) std::rethrow_exception(std::move(eptr));
                throw_error(error::logic, "unexpected null eptr");
            },
            [](Response response) -> Response {
                return std::move(response.value);
            }
        },
        std::move(handler_result)
    );
}

SubscriberId Client::new_subscriber_id() {
    return _state->next_subscriber_id++;
}

void Client::subscribe(const RepositoryHandle& repo_handle, SubscriberId subscriber_id, std::function<HandlerSig> handler, asio::yield_context yield) {
    if (!_state) {
        throw_error(error::not_connected);
    }

    auto message_id = _state->subscriptions.subscribe(repo_handle, subscriber_id, std::move(handler), _state->next_request_id);

    if (message_id) {
        _state->pending.emplace(std::pair(message_id->value, Subscription{}));

        auto request = Request::RepositorySubscribe {
            repo_handle,
        };

        send(_state->socket, *message_id, request, yield);
    }
}

void Client::unsubscribe(const RepositoryHandle& repo_handle, SubscriberId subscriber_id, asio::yield_context yield) {
    if (!_state) {
        throw_error(error::not_connected);
    }

    auto message_id = _state->subscriptions.unsubscribe(repo_handle, subscriber_id);

    if (message_id) {
        auto request = Request::SessionUnsubscribe {
            MessageId{message_id->value}
        };

        invoke(request, yield);
    }
}

/**
 * Client connects to the Ouisync server over a TCP endpoint on the below
 * `port` and the connection is authenticated using `auth_key`.
 */
struct LocalEndpoint {
    uint16_t port;
    std::vector<uint8_t> auth_key;
};

static
LocalEndpoint read_local_endpoint(const boost::filesystem::path& config_dir_path) {
    boost::filesystem::path config_path = config_dir_path / "local_endpoint.conf";
    std::ifstream config_file;
    config_file.open(config_path);

    if (!config_file.is_open()) {
        throw_error(error::connect, "Could not open file " + config_path.string());
    }

    std::stringstream buffer;
    buffer << config_file.rdbuf();

    namespace js = boost::json;

    js::object obj = js::parse(buffer.str()).as_object();

    int64_t port = obj["port"].as_int64();

    if (port <= 0 || port > std::numeric_limits<uint16_t>::max()) {
        throw_error(error::connect, "invalid port");
    }

    js::string auth_key_hex = obj["auth_key"].as_string();

    std::vector<uint8_t> auth_key;
    boost::algorithm::unhex(auth_key_hex, std::back_inserter(auth_key));

    return LocalEndpoint{uint16_t(port), std::move(auth_key)};
}

static
void authenticate(Socket& socket, const std::vector<uint8_t>& auth_key, asio::yield_context yield) {
    using Hmac = boost::hash2::hmac_sha2_256;
    using Digest = Hmac::result_type;

    const uint16_t challenge_size = 256;
    const uint16_t proof_size = Digest().size();

    // Generate client's challenge
    std::random_device dev;
    std::mt19937 rng(dev());
    std::uniform_int_distribution<std::mt19937::result_type> dist(0, 255);
    std::vector<uint8_t> client_challenge(challenge_size);
    std::generate(client_challenge.begin(), client_challenge.end(), [&] () { return dist(rng); });

    // Authenticate server to the client:
    // * Send client_challenge
    // * Receive server_proof
    // * Ensure server_proof == HMAC(auth_key ++ client_challenge)
    asio::async_write(socket, asio::buffer(client_challenge), yield);

    std::vector<uint8_t> server_proof(proof_size);
    asio::async_read(socket, asio::buffer(server_proof), yield);

    Hmac hmac(auth_key.data(), auth_key.size());
    hmac.update(client_challenge.data(), client_challenge.size());

    if (!std::ranges::equal(hmac.result(), server_proof)) {
        throw_error(error::auth, "Server failed to authenticate");
    }

    // Authenticate client to the server:
    // * Receive server_challenge
    // * Send client_proof = HMAC(auth_key ++ server_challenge)
    std::vector<uint8_t> server_challenge(challenge_size);
    asio::async_read(socket, asio::buffer(server_challenge), yield);
    hmac = Hmac(auth_key.data(), auth_key.size());
    hmac.update(server_challenge.data(), server_challenge.size());
    Digest client_proof = hmac.result();

    asio::async_write(socket, asio::buffer(client_proof), yield);
}

// static
Client Client::connect(
    const boost::filesystem::path& config_dir_path,
    asio::yield_context yield
) {
    auto ep = read_local_endpoint(config_dir_path);

    Socket socket(yield.get_executor());

    socket.async_connect(
        asio::ip::tcp::endpoint(
            asio::ip::address_v4::loopback(),
            ep.port
        ),
        yield
    );

    authenticate(socket, ep.auth_key, yield);

    return Client(std::make_shared<State>(std::move(socket)));
}

} // namespace ouisync
