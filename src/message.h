#pragma once

#include "shortcuts.h"
#include "cancel.h"
#include "version_vector.h"
#include "commit.h"
#include "variant.h"
#include "directory.h"
#include "file_blob.h"
#include "branch.h"

#include <boost/asio/ip/tcp.hpp>
#include <boost/asio/awaitable.hpp>

namespace ouisync {

enum class MessageType { Request, Response };

struct RqNotifyOnChange {
    static constexpr auto type = MessageType::Request;

    Opt<uint64_t> last_state;

    template<class Archive>
    void serialize(Archive& ar, const unsigned) {
        ar & last_state;
    }
};

struct RsNotifyOnChange {
    static constexpr auto type = MessageType::Response;

    uint64_t new_state;

    template<class Archive>
    void serialize(Archive& ar, const unsigned) {
        ar & new_state;
    }
};

struct RqIndex {
    static constexpr auto type = MessageType::Request;

    template<class Archive>
    void serialize(Archive& ar, const unsigned) {
    }
};

struct RsIndex {
    static constexpr auto type = MessageType::Response;

    Index index;

    template<class Archive>
    void serialize(Archive& ar, const unsigned) {
        ar & index;
    }
};

struct RqObject {
    static constexpr auto type = MessageType::Request;

    ObjectId object_id;

    template<class Archive>
    void serialize(Archive& ar, const unsigned) {
        ar & object_id;
    }
};

struct RsObject {
    static constexpr auto type = MessageType::Response;

    using Object = variant<FileBlob, Directory>;

    // boost::none if not found
    Opt<Object> object;

    template<class Archive>
    void serialize(Archive& ar, const unsigned) {
        ar & object;
    }
};

namespace MessageDetail {
    using MessageVariant
        = variant<
            RqNotifyOnChange,
            RsNotifyOnChange,
            RqIndex,
            RsIndex,
            RqObject,
            RsObject
        >;

    using RequestVariant
        = variant<
            RqNotifyOnChange,
            RqIndex,
            RqObject
        >;

    using ResponseVariant
        = variant<
            RsNotifyOnChange,
            RsIndex,
            RsObject
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

std::ostream& operator<<(std::ostream&, const RqNotifyOnChange&);
std::ostream& operator<<(std::ostream&, const RsNotifyOnChange&);
std::ostream& operator<<(std::ostream&, const RqIndex&);
std::ostream& operator<<(std::ostream&, const RsIndex&);
std::ostream& operator<<(std::ostream&, const RqObject&);
std::ostream& operator<<(std::ostream&, const RsObject&);
std::ostream& operator<<(std::ostream&, const Request&);
std::ostream& operator<<(std::ostream&, const Response&);
std::ostream& operator<<(std::ostream&, const Message&);

} // namespace
