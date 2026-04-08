#include "ouisync/data.g.hpp"
#include "ouisync/message.g.hpp"
#include <boost/asio/detached.hpp>
#include <boost/asio/spawn.hpp>
#include <boost/system/detail/error_code.hpp>
#include <exception>
#include <ouisync/client.hpp>
#include <ouisync/serialize.hpp>
#include <ouisync/debug.hpp> // debug
#include <ouisync/error.hpp>
#include <ouisync/utils.hpp>
#include <ouisync/semaphore.hpp>

#include <boost/json.hpp>
#include <boost/hash2/sha2.hpp>
#include <boost/asio/experimental/channel.hpp>
#include <boost/asio/ip/tcp.hpp>
#include <boost/asio/write.hpp>
#include <boost/asio/read.hpp>
#include <boost/json/src.hpp>

#include <ranges>
#include <fstream>
#include <random>

#include <unordered_map>
#include <variant>

namespace ouisync {

namespace asio = boost::asio;
namespace system = boost::system;
namespace endian = boost::endian;

using Socket = asio::ip::tcp::socket;
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
                              handler = std::move(handler)
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
        auto bound_handler = [
            handler = std::move(handler),
            ec
        ] () mutable {
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
    // Set up response handler *before* we send the request.
    state->responses.emplace(std::pair(msg_id.value, std::move(handler)));

    state->send(
        msg_id,
        std::move(request),
        [state, msg_id](system::error_code ec) mutable {
            if (ec) {
                auto i = state->responses.find(msg_id.value);
                if (i != state->responses.end()) {
                    (i->second)(ec, Response {});
                }
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
        Request::SessionUnsubscribe { subscribe_id },
        [](boost::system::error_code ec, Response res) {
            if (ec) {
                std::cerr << "failed to unsubscribe: " << ec << std::endl;
                return;
            }

            if (res.get_if<Response::Unit>() == nullptr) {
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
