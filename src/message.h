#pragma once

#include "shortcuts.h"
#include "cancel.h"
#include "version_vector.h"
#include "commit.h"
#include "variant.h"

#include <boost/asio/ip/tcp.hpp>
#include <boost/asio/awaitable.hpp>

namespace ouisync {

enum class MessageType { Request, Response };

struct RqHeads {
    static constexpr MessageType type = MessageType::Request;

    template<class Archive>
    void serialize(Archive&, const unsigned int) {}
};

struct RsHeads : std::vector<Commit> {
    static constexpr MessageType type = MessageType::Response;

    using Parent = std::vector<Commit>;
    using Parent::Parent;
    using Parent::begin;
    using Parent::end;

    template<class Archive>
    void serialize(Archive& ar, const unsigned int) {
        ar & static_cast<Parent&>(*this);
    }
};

namespace MessageDetail {
    using MessageVariant
        = variant<
            RqHeads,
            RsHeads
        >;

    using RequestVariant
        = variant<
            RqHeads
        >;

    using ResponseVariant
        = variant<
            RsHeads
        >;
} // MessageDetail namespace

struct Request : MessageDetail::RequestVariant
{
    using Variant = MessageDetail::RequestVariant;
    using Variant::Variant;
};

struct Response : MessageDetail::ResponseVariant
{
    using Variant = MessageDetail::ResponseVariant;
    using Variant::Variant;
};

struct Message : MessageDetail::MessageVariant
{
    using Variant = MessageDetail::MessageVariant;
    using Variant::Variant;

    Message(Request r) {
        apply(r, [&] (auto& r) { static_cast<Variant&>(*this) = std::move(r); });
    }

    Message(Response r) {
        apply(r, [&] (auto& r) { static_cast<Variant&>(*this) = std::move(r); });
    }

    static
    net::awaitable<Message> receive(net::ip::tcp::socket&, Cancel);

    static
    net::awaitable<void> send(net::ip::tcp::socket&, const Message&, Cancel);

    template<class Archive>
    void serialize(Archive& ar, const unsigned int) {
        ar & static_cast<Variant&>(*this);
    }
};

std::ostream& operator<<(std::ostream&, const RqHeads&);
std::ostream& operator<<(std::ostream&, const RsHeads&);
std::ostream& operator<<(std::ostream&, const Request&);
std::ostream& operator<<(std::ostream&, const Response&);
std::ostream& operator<<(std::ostream&, const Message&);

} // namespace
