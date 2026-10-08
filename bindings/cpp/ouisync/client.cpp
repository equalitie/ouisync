#include "ouisync/data.g.hpp"
#include "ouisync/message.g.hpp"
#include <boost/asio/any_completion_handler.hpp>
#include <boost/asio/any_io_executor.hpp>
#include <boost/asio/associated_cancellation_slot.hpp>
#include <boost/asio/cancellation_type.hpp>
#include <boost/asio/detached.hpp>
#include <boost/asio/error.hpp>
#include <boost/asio/spawn.hpp>
#include <boost/system/detail/error_code.hpp>
#include <boost/system/system_error.hpp>
#include <exception>
#include <ouisync/client.hpp>
#include <ouisync/serialize.hpp>
#include <ouisync/debug.hpp> // debug
#include <ouisync/error.hpp>
#include <ouisync/utils.hpp>
#include <ouisync/semaphore.hpp>

#include <boost/algorithm/hex.hpp>
#include <boost/json.hpp>
#include <boost/hash2/sha2.hpp>
#include <boost/asio/experimental/channel.hpp>
#include <boost/asio/generic/stream_protocol.hpp>
#include <boost/asio/ip/tcp.hpp>
#include <boost/asio/local/stream_protocol.hpp>
#include <boost/asio/write.hpp>
#include <boost/filesystem/operations.hpp>
#include <boost/asio/read.hpp>
#include <boost/json/src.hpp>

#include <charconv>
#include <optional>
#include <ranges>
#include <fstream>
#include <random>

#include <unordered_map>
#include <variant>

namespace ouisync {

namespace asio = boost::asio;
namespace system = boost::system;
namespace endian = boost::endian;

// Protocol-agnostic stream socket. Can be connected either over TCP or over a unix domain socket.
using Socket = asio::generic::stream_protocol::socket;
using ResponseHandler = asio::any_completion_handler<HandlerSig>;
using RawMessageId = decltype(MessageId::value);

// Data whose lifetime needs to be preserved while sending operation takes
// place.
struct SendBufferData {
    uint32_t rq_size_be;
    uint64_t rq_id_be;
    std::stringstream serialized_rq;

    void prepare(MessageId rq_id, const Request& rq) {
        serialized_rq = serialize(rq);

        rq_id_be = endian::native_to_big(rq_id.value);
        uint32_t rq_size = sizeof(rq_id_be) + serialized_rq.view().size();
        rq_size_be = endian::native_to_big(rq_size);
    }

    std::array<asio::const_buffer, 3> to_buffers() const {
        return std::array<asio::const_buffer, 3>{
            asio::buffer(&rq_size_be, sizeof(rq_size_be)),
            asio::buffer(&rq_id_be, sizeof(rq_id_be)),
            asio::buffer(serialized_rq.view())
        };
    }
};

struct Client::State : std::enable_shared_from_this<Client::State> {
    Socket socket;
    std::unordered_map<RawMessageId, ResponseHandler> responses;
    std::unordered_map<RawMessageId, std::shared_ptr<detail::SubscriptionChannel>> subscriptions;

    RawMessageId next_message_id = 0;
    bool disconnected = false;

    // Ensures only one socket write happens at a time
    Semaphore send_semaphore;
    SendBufferData send_buffer_data;

    State(Socket socket) :
        socket(std::move(socket)),
        send_semaphore(this->socket.get_executor())
    {}

    template<
        asio::completion_token_for<void(system::error_code)> CompletionToken
    >
    auto send(MessageId rq_id, Request rq, CompletionToken&& token) {
        return boost::asio::async_initiate<CompletionToken, void(system::error_code)>(
            [ state = shared_from_this(), rq_id, rq = std::move(rq)](auto handler) {
                state->send_semaphore.get_permit(
                    [ state, rq_id, rq = std::move(rq), handler = std::move(handler) ]
                    (system::error_code ec, Semaphore::Permit permit) mutable {
                        if (ec) {
                            handler(ec);
                            return;
                        }
                        state->send_buffer_data.prepare(rq_id, rq);
                        asio::async_write(state->socket, state->send_buffer_data.to_buffers(),
                            [ permit = std::move(permit),
                              handler = std::move(handler),
                              // ensure `state->send_buffer_data` is alive until the write completes.
                              state
                            ] (system::error_code ec, size_t) mutable {
                                handler(ec);
                            });
                    }
                );
            },
            token
        );
    }
};

HandlerResult to_handler_result(ResponseResult&& rs) {
    return std::visit(overloaded {
        [](ResponseResult::Failure&& failure) -> HandlerResult {
            // TODO: We're losing useful `message` and `sources` debug
            // information.
            return HandlerResult(make_error_code(failure.code));
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
    boost::system::error_code ec;

    try {
        while (state->socket.is_open()) {
            auto [res_id, res] = receive(state->socket, yield);

            auto res_i = state->responses.find(res_id.value);
            if (res_i != state->responses.end()) {
                auto& handler = res_i->second;
                auto bound_handler = [
                    handler = std::move(handler),
                    res = std::move(res)
                ] () mutable {
                    apply_result(std::move(handler), std::move(res));
                };

                state->responses.erase(res_i);

                asio::post(yield.get_executor(), std::move(bound_handler));

                continue;
            }

            auto sub_i = state->subscriptions.find(res_id.value);
            if (sub_i != state->subscriptions.end()) {
                auto& sub = sub_i->second;

                std::visit(overloaded {
                    [&](Response res) {
                        if (res.get_if<Response::None>() == nullptr) {
                            sub->async_send(boost::system::error_code(), res, yield);
                        } else {
                            sub->close();
                            state->subscriptions.erase(sub_i);
                        }
                    },
                    [&](boost::system::error_code ec) {
                        sub->async_send(ec, Response {}, yield);
                    },
                }, res);
            }
        }
    } catch (boost::system::system_error const& e) {
        ec = e.code();
    } catch (...) {
        ec = error::logic;
    }

    auto responses = std::move(state->responses);
    auto subscriptions = std::move(state->subscriptions);

    for (auto& [res_id, handler] : responses) {
        auto bound_handler = [handler = std::move(handler), ec] () mutable {
            apply_result(std::move(handler), ec);
        };

        asio::post(yield.get_executor(), std::move(bound_handler));
    }

    for (auto& [msg_id, sub] : subscriptions) {
        sub->close();
    }

    state->disconnected = true;
}

Client::Client(std::shared_ptr<State>&& state)
    : _state(std::move(state))
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
                std::cerr << "Uncaught exception: " << e.what() << std::endl;
                std::terminate();
            }
            catch (...) {
                std::cerr << "Uncaught exception: unknown" << std::endl;
                std::terminate();
            }
        }
    );
}

Client::~Client() {
    if (!is_connected()) {
        return;
    }

    auto& socket = _state->socket;

    if (socket.is_open()) {
        socket.close();
    }
}

/* static */
void Client::invoke_impl(
        std::shared_ptr<State> state,
        MessageId msg_id,
        Request request,
        asio::any_completion_handler<HandlerSig> handler)
{
    if (state->disconnected) {
        handler(asio::error::shut_down, Response {});
        return;
    }

    // Handle cancellation
    auto cancellation_slot = handler.get_cancellation_slot();
    if (cancellation_slot.is_connected()) {
        cancellation_slot.assign([state, msg_id](asio::cancellation_type) {
            auto i = state->responses.find(msg_id.value);
            if (i == state->responses.end()) {
                return;
            }

            auto handler = std::move(i->second);
            state->responses.erase(i);

            Client::invoke_impl(
                state,
                MessageId { state->next_message_id++ },
                Request::Cancel { msg_id },
                [handler = std::move(handler)](system::error_code ec, Response response) mutable {
                    // If the `Cancel` request itself failed, then invoke the handler with that
                    // error.
                    if (ec) {
                        handler(ec, Response {});
                        return;
                    }

                    // `response` should be a boolean indicating whether the operation was actually
                    // cancelled or whether it already completed before the cancel request has been
                    // processed. We return `operation_aborted` error in both case because we
                    // already deregistered the completion handler and so there is no way to observe
                    // the result of the operation anyway. We just verify the response is the
                    // correct type.
                    if (response.get_if<Response::Bool>() == nullptr) {
                        handler(error::protocol, Response {});
                        return;
                    }

                    handler(asio::error::operation_aborted, Response {});
                }
            );
        });
    }

    // Set up response handler *before* we send the request.
    state->responses.emplace(std::pair(msg_id.value, std::move(handler)));

    state->send(
        msg_id,
        std::move(request),
        [state, msg_id](system::error_code ec) mutable {
            if (ec) {
                auto i = state->responses.find(msg_id.value);
                if (i == state->responses.end()) {
                    return;
                }

                auto handler = std::move(i->second);
                state->responses.erase(i);

                handler(ec, Response {});
            }
        }
    );
}

void Client::subscribe_impl(MessageId subscribe_id, Request request, std::shared_ptr<detail::SubscriptionChannel> channel) {
    _state->subscriptions.emplace(std::pair(subscribe_id.value, std::move(channel)));

    Client::invoke_impl(
        _state,
        subscribe_id,
        request,
        [state = _state, subscribe_id](boost::system::error_code ec, Response res) {
            if (!ec && res.get_if<Response::Unit>() == nullptr) {
                ec = error::protocol;
            }

            if (!ec) {
                return;
            }

            // Failed to create the subscription - unregister the channel, send the error on it and
            // close it.
            auto i = state->subscriptions.find(subscribe_id.value);
            if (i == state->subscriptions.end()) {
                return;
            }

            auto channel = std::move(i->second);
            state->subscriptions.erase(i);

            asio::spawn(
                state->socket.get_executor(),
                [channel = std::move(channel), ec](asio::yield_context yield) {
                    channel->async_send(ec, Response {}, yield);
                    channel->close();
                },
                asio::detached
            );
        }
    );
}

void Client::unsubscribe_impl(MessageId subscribe_id) {
    auto i = _state->subscriptions.find(subscribe_id.value);
    if (i != _state->subscriptions.end()) {
        _state->subscriptions.erase(i);
    }

    auto unsubscribe_id = next_message_id();

    Client::invoke_impl(
        _state,
        unsubscribe_id,
        Request::Cancel { subscribe_id },
        [](boost::system::error_code ec, Response res) {
            if (ec) {
                std::cerr << "failed to unsubscribe: " << ec << std::endl;
                return;
            }

            if (res.get_if<Response::Bool>() == nullptr) {
                std::cerr << "failed to unsubscribe: unexpected response" << std::endl;
            }
        }
    );
}

bool Client::is_connected() const noexcept {
    return _state && !_state->disconnected;
}

asio::any_io_executor Client::get_executor() const {
    if (!_state) {
        throw_error(error::logic, "client has no state");
    }
    return _state->socket.get_executor();
}

MessageId Client::next_message_id() {
    return MessageId { _state->next_message_id++ };
}

/**
 * Address of the local Ouisync service. It's either a unix domain socket (authenticated via file
 * permissions) or a TCP endpoint (authenticated by explicit handshake using `auth_key`).
 */
struct ServiceAddress {
    asio::generic::stream_protocol::endpoint endpoint;
    std::optional<std::vector<uint8_t>> auth_key;
};

[[noreturn]] static
void throw_invalid_address(std::string_view raw, std::string_view reason) {
    throw system::system_error(
        make_error_code(error::invalid_service_address),
        "invalid service address: " + std::string(raw) + " - " + std::string(reason)
    );
}

// Parses TCP endpoint in the form `ADDR:PORT` (`[ADDR]:PORT` for IPv6). The port is required and
// must be non-zero.
static
asio::ip::tcp::endpoint parse_tcp_endpoint(std::string_view raw) {
    system::error_code ec;

    // Bare address without port (also catches unbracketed IPv6 address, which would otherwise be
    // ambiguously split on its last colon).
    asio::ip::make_address(std::string(raw), ec);
    if (!ec || raw.ends_with(']')) {
        throw_invalid_address(raw, "missing port");
    }

    auto colon = raw.rfind(':');
    if (colon == std::string_view::npos) {
        throw_invalid_address(raw, "invalid socket address");
    }

    auto raw_host = raw.substr(0, colon);
    auto raw_port = raw.substr(colon + 1);

    if (raw_host.starts_with('[') && raw_host.ends_with(']')) {
        raw_host = raw_host.substr(1, raw_host.size() - 2);
    }

    auto addr = asio::ip::make_address(std::string(raw_host), ec);
    if (ec) {
        throw_invalid_address(raw, "invalid ip address");
    }

    uint16_t port = 0;
    auto [ptr, port_ec] = std::from_chars(raw_port.data(), raw_port.data() + raw_port.size(), port);
    if (port_ec != std::errc() || ptr != raw_port.data() + raw_port.size()) {
        throw_invalid_address(raw, "invalid port");
    }

    if (port == 0) {
        throw_invalid_address(raw, "port must not be zero");
    }

    return asio::ip::tcp::endpoint(addr, port);
}

// Parses TCP service address in the form `tcp://ADDR:PORT?auth_key=HEX`.
static
ServiceAddress parse_tcp_service_address(std::string_view raw) {
    constexpr std::string_view scheme = "tcp://";
    constexpr std::string_view auth_key_param = "?auth_key=";

    if (!raw.starts_with(scheme)) {
        throw_invalid_address(raw, "unsupported scheme");
    }

    auto rest = raw.substr(scheme.size());

    auto param_pos = rest.find(auth_key_param);
    if (param_pos == std::string_view::npos) {
        throw_invalid_address(raw, "missing auth_key query parameter");
    }

    auto endpoint = parse_tcp_endpoint(rest.substr(0, param_pos));

    std::vector<uint8_t> auth_key;

    try {
        boost::algorithm::unhex(
            rest.substr(param_pos + auth_key_param.size()),
            std::back_inserter(auth_key)
        );
    } catch (const boost::algorithm::hex_decode_error&) {
        throw_invalid_address(raw, "invalid auth_key");
    }

    return ServiceAddress { endpoint, std::move(auth_key) };
}

static
ServiceAddress read_service_address(const boost::filesystem::path& config_dir_path) {
#if defined(BOOST_ASIO_HAS_LOCAL_SOCKETS)
    boost::filesystem::path unix_socket_path = config_dir_path / "local_endpoint.sock";
    if (boost::filesystem::exists(unix_socket_path)) {
        return ServiceAddress {
            asio::local::stream_protocol::endpoint(unix_socket_path.string()),
            std::nullopt,
        };
    }
#endif

    boost::filesystem::path conf_path = config_dir_path / "local_endpoint.conf";
    std::ifstream conf_file;
    conf_file.open(conf_path);

    if (!conf_file.is_open()) {
        throw_error(error::service_config_not_found, "Could not open file " + conf_path.string());
    }

    std::stringstream buffer;
    buffer << conf_file.rdbuf();

    namespace js = boost::json;

    system::error_code ec;
    js::value value = js::parse(buffer.str(), ec);
    if (ec) {
        throw_invalid_address(buffer.str(), "invalid json");
    }

    auto raw = value.if_string();

    if (raw == nullptr) {
        throw_invalid_address(buffer.str(), "not a string");
    }

    constexpr std::string_view unix_scheme = "unix://";

    if (raw->starts_with(unix_scheme)) {
#if defined(BOOST_ASIO_HAS_LOCAL_SOCKETS)
        // Relative path is resolved against the config dir.
        boost::filesystem::path path(std::string(raw->subview(unix_scheme.size())));
        if (path.is_relative()) {
            path = config_dir_path / path;
        }

        return ServiceAddress {
            asio::local::stream_protocol::endpoint(path.string()),
            std::nullopt,
        };
#else
        throw_invalid_address(*raw, "unix domain sockets not supported on this platform");
#endif
    }

    return parse_tcp_service_address(*raw);
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

static std::shared_ptr<Client> connect_coro(ServiceAddress addr, asio::yield_context yield) {
    Socket socket(yield.get_executor());

    socket.async_connect(addr.endpoint, yield);

    if (addr.auth_key) {
        authenticate(socket, *addr.auth_key, yield);
    }

    return std::make_shared<Client>(
        std::make_shared<Client::State>(std::move(socket))
    );
};

// static
void Client::connect_impl(
    const boost::asio::any_io_executor& exec,
    const boost::filesystem::path& config_dir_path,
    asio::any_completion_handler<void(system::error_code, std::shared_ptr<Client>)> handler
) {
    // NOTE: Everything that can fail must happen inside the coroutine and the errors must be
    // passed to the handler. Throwing from here (the initiation function) would bypass the
    // caller's try/catch when using `yield_context`.
    asio::spawn(
        exec,
        [config_dir_path, handler = std::move(handler)]
        (asio::yield_context yield) mutable {
            system::error_code ec;
            std::shared_ptr<Client> client;

            try {
                auto addr = read_service_address(config_dir_path);
                client = connect_coro(std::move(addr), yield[ec]);
            } catch (const system::system_error& e) {
                // The completion signature only carries the error code, so log the details.
                std::cerr << "Failed to connect to Ouisync service: " << e.what() << std::endl;
                ec = e.code();
            }

            handler(ec, std::move(client));
        },
        asio::detached
    );
}

} // namespace ouisync
