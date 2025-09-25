#include <ouisync/client.hpp>
#include <ouisync/serialize.hpp>
#include <ouisync/debug.hpp> // debug
#include <ouisync/error.hpp>

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
using HandlerSig = void(std::exception_ptr, ResponseResult);
// If this breaks due to being in the detail namespace, consider using
// asio::any_completion_handler<HandlerSig>
using Handler = asio::detail::spawn_handler<asio::any_io_executor, HandlerSig, void>;
using RawMessageId = decltype(MessageId::value);

// Utility for using std::variant
template<class... Ts>
struct overloaded : Ts... { using Ts::operator()...; };

struct Empty {};
using ResponseEntry = std::variant<Empty, ResponseResult, std::exception_ptr, Handler>;

struct Client::State {
    Socket socket;
    std::unordered_map<RawMessageId, ResponseEntry> pending;
    RawMessageId next_request_id = 0;
};

static
void send(Socket& socket, RawMessageId rq_id, const Request& rq, asio::yield_context yield) {
    std::stringstream request_buffer = serialize(rq);

    uint64_t rq_id_be = endian::native_to_big(rq_id);

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

static
std::tuple<RawMessageId, ResponseResult>
receive(Socket& socket, asio::yield_context yield) {
    uint32_t rs_size_be;
    asio::async_read(socket, asio::buffer(&rs_size_be, sizeof(rs_size_be)), yield);
    auto rs_size = endian::big_to_native(rs_size_be);

    uint64_t rs_id_be;

    if (rs_size < sizeof(rs_id_be)) {
        throw_exception(error::protocol, "response too small");
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
        endian::big_to_native(rs_id_be),
        std::move(response_result)
    };
}

/* static */
void Client::receive_job(std::shared_ptr<State> state, boost::asio::yield_context yield) {
    try {
        while (state->socket.is_open()) {
            auto [rs_id, rs] = receive(state->socket, yield);
            auto entry_i = state->pending.find(rs_id);
            if (entry_i == state->pending.end()) {
                continue;
            }
            auto& entry = entry_i->second;

            std::visit(overloaded {
                [&](Empty) {
                    entry.emplace<ResponseResult>(std::move(rs));
                },
                [&](const ResponseResult&) {
                    std::cout << "Warning: response received more than once (ignored)\n";
                },
                [&](const std::exception_ptr&) {
                    std::cout << "Warning: response received more than once (ignored)\n";
                },
                [&](Handler& handler) {
                    auto h = [
                        handler = std::move(handler),
                        rs = std::move(rs)
                    ] () mutable {
                        std::move(handler)(std::exception_ptr(), std::move(rs));
                    };
                    state->pending.erase(entry_i);
                    asio::post(yield.get_executor(), std::move(h));
                },
            },
            entry);
        }
    }
    catch (...) {
        std::exception_ptr eptr = std::current_exception();
        auto pending = std::move(state->pending);

        for (auto& [rq_id, entry] : state->pending) {
            std::visit(overloaded {
                [&](Empty) {
                    entry = eptr;
                },
                [&](const ResponseResult&) {
                    // Ignored, user will receive original response
                },
                [&](const std::exception_ptr&) {
                    // Ignored, user will receive original error
                },
                [&](Handler& handler) {
                    auto h = [
                        handler = std::move(handler),
                        eptr
                    ] () mutable {
                        std::move(handler)(eptr, ResponseResult{});
                    };
                    asio::post(yield.get_executor(), std::move(h));
                },
            },
            entry);
        }
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
        [](std::exception_ptr e) {
            // We're catching exceptions in the `receive_job`
            assert(!e);
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
    state->pending.emplace(std::pair(rq_id, Empty{}));

    return asio::async_initiate<CompletionToken, HandlerSig>(
        // Note that this lambda is executed only once the token is awaited.
        [state = std::move(state), rq_id](auto handler) {
            auto entry_i = state->pending.find(rq_id);
            auto& entry = entry_i->second;
            std::visit(overloaded {
                [&](Empty) {
                    entry.emplace<Handler>(std::move(handler));
                },
                [&](ResponseResult& rs) {
                    auto rs_ = std::move(rs);
                    state->pending.erase(entry_i);
                    std::move(handler)({}, std::move(rs_));
                },
                [&](std::exception_ptr& eptr) {
                    auto eptr_ = std::move(eptr);
                    state->pending.erase(entry_i);
                    std::move(handler)(std::move(eptr), {});
                },
                [&](Handler&) {
                    throw_exception(error::logic, "Handler already set");
                },
            },
            entry);
        },
        token);
}

Response Client::invoke(const Request& request, asio::yield_context yield) {
    auto rq_id = _state->next_request_id++;

    auto responded = add_pending(_state, rq_id, asio::deferred);
    send(_state->socket, rq_id, request, yield);
    ResponseResult result = responded(yield);

    // Handle service error
    if (auto failure = std::get_if<ResponseResult::Failure>(&result.value)) {
        boost::system::system_error e(failure->code, failure->message);
        throw e;
    }

    return std::move(std::get<Response>(result.value));
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
        throw_exception(error::connect, "Could not open file " + config_path.string());
    }

    std::stringstream buffer;
    buffer << config_file.rdbuf();

    namespace js = boost::json;

    js::object obj = js::parse(buffer.str()).as_object();

    int64_t port = obj["port"].as_int64();

    if (port <= 0 || port > std::numeric_limits<uint16_t>::max()) {
        throw_exception(error::connect, "invalid port");
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
        throw_exception(error::auth, "Server failed to authenticate");
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
