#include "message.h"
#include "error.h"
#include "archive.h"

#include <boost/endian/conversion.hpp>

#include <boost/asio/read.hpp>
#include <boost/asio/write.hpp>
#include <boost/asio/use_awaitable.hpp>

#include <boost/optional/optional_io.hpp>

#include <boost/serialization/optional.hpp>
#include <boost/serialization/variant.hpp>
#include <boost/serialization/vector.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/set.hpp>
#include <boost/serialization/array.hpp>


#include <algorithm>

static const size_t MAX_MESSAGE_SIZE = 1 << 20; // 1MB

using namespace ouisync;
using net::ip::tcp;

/* static */
net::awaitable<Message> Message::receive(tcp::socket& socket, Cancel cancel)
try {
    auto close_socket = cancel.connect([&] { socket.close(); });

    uint32_t size = 0;
    co_await net::async_read(socket, net::buffer(&size, sizeof(size)), net::use_awaitable);
    if (cancel) throw_error(net::error::operation_aborted);

    boost::endian::big_to_native_inplace(size);

    if (size > MAX_MESSAGE_SIZE) throw_error(net::error::message_size);

    std::string rx_buf(size, '\n'); // XXX: Reuse

    co_await net::async_read(socket, net::buffer(rx_buf), net::use_awaitable);
    if (cancel) throw_error(net::error::operation_aborted);

    std::stringstream ss(move(rx_buf));

    InputArchive ia(ss);
    Message m;
    ia >> m;

    co_return m;
}
catch (const std::exception& e) {
    if (cancel) throw_error(net::error::operation_aborted);
    throw;
}

/* static */
net::awaitable<void> Message::send(tcp::socket& socket, const Message& message, Cancel cancel)
try {
    auto close_socket = cancel.connect([&] { socket.close(); });

    std::stringstream ss;
    OutputArchive oa(ss);
    oa << message;
    std::string ms = move(*ss.rdbuf()).str();
    assert(ms.size() < MAX_MESSAGE_SIZE);

    uint32_t size = ms.size();
    boost::endian::native_to_big_inplace(size);

    co_await net::async_write(socket, net::buffer(&size, sizeof(size)), net::use_awaitable);
    if (cancel) throw_error(net::error::operation_aborted);

    co_await net::async_write(socket, net::buffer(ms), net::use_awaitable);
    if (cancel) throw_error(net::error::operation_aborted);
}
catch (const std::exception& e) {
    if (cancel) throw_error(net::error::operation_aborted);
    throw;
}

std::ostream& ouisync::operator<<(std::ostream& os, const RqNotifyOnChange& rq) {
    return os << "RqNotifyOnChange{}";
}

std::ostream& ouisync::operator<<(std::ostream& os, const RsNotifyOnChange& m) {
    return os << "RsNotifyOnChange{}";
}

std::ostream& ouisync::operator<<(std::ostream& os, const RqIndices& rq) {
    return os << "RqIndices{}";
}

std::ostream& ouisync::operator<<(std::ostream& os, const RsIndices& m) {
    return os << "RsIndices{...}";
}

std::ostream& ouisync::operator<<(std::ostream& os, const RqObject& rq) {
    return os << "RqObject " << rq.object_id;
}

std::ostream& ouisync::operator<<(std::ostream& os, const RsObject& m) {
    return os << "RsObject{" << m.object << "}";
}

std::ostream& ouisync::operator<<(std::ostream& os, const Request& m) {
    apply(m, [&os] (auto& m) { os << m; });
    return os;
}

std::ostream& ouisync::operator<<(std::ostream& os, const Response& m) {
    apply(m, [&os] (auto& m) { os << m; });
    return os;
}

std::ostream& ouisync::operator<<(std::ostream& os, const Message& m) {
    apply(m, [&os] (auto& m) { os << m; });
    return os;
}
