#pragma once

#include <ouisync/handler.hpp>
#include <ouisync/error.hpp>
#include <ouisync/subscriber_id.hpp>
#include <unordered_map>
#include <unordered_set>
#include <boost/asio/any_io_executor.hpp>

namespace ouisync {

//
// Table for managing subscriptions.
//
// How it works:
//
// Let's have a subscriber with ID `subscriber_id` who wants to get notified on
// changes of repository `repo_id`.
//
// The subscriber sends a subscription request with message ID `message_id`.
//
// Then whenever the client receives a response with `message_id`, the
// subscriber gets notified.
//
// Once there is one subscriber to `repo_id`, all other subscribers to that
// `repo_id` shall attach their handlers to the existing subscription whith
// `message_id` instead of sending new subscription requests.
//
// Invariant [SUB_TABLE]:
//   by_message_id[msg_id][subscriber_id] exists
//      <=> by_subscriber_id[subscriber_id] exists
//      <=> by_repo_handle[repo_id][subscriber_id] exists
//
class Subscriptions {
private:
    using RawMessageId = decltype(MessageId::value);
    using RawRepositoryHandle = decltype(RepositoryHandle::value);

    using ByMessageId = std::unordered_map<RawMessageId, std::unordered_set<SubscriberId>>;
    using BySubscriberId = std::unordered_map<SubscriberId, std::pair<RawMessageId, std::function<HandlerSig>>>;
    using ByRepoHandle = std::unordered_map<RawRepositoryHandle, RawMessageId>;

    ByMessageId by_message_id;
    BySubscriberId by_subscriber_id;
    ByRepoHandle by_repo_handle;

public:
    // Return message ID for sending a new subscription request, if needed.
    std::optional<MessageId> subscribe(
        RepositoryHandle,
        SubscriberId,
        std::function<HandlerSig> handler,
        RawMessageId& next_message_id
    );

    // Return message ID for sending an unsubscribe request, if needed.
    std::optional<MessageId> unsubscribe(
        RepositoryHandle,
        SubscriberId
    );

    void handle(
        boost::asio::any_io_executor,
        MessageId,
        const HandlerResult&
    );

    void handle_all(boost::asio::any_io_executor, std::exception_ptr);

    bool is_subscribed(SubscriberId);
};

} // namespace ouisync
