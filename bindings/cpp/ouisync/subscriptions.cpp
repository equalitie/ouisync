#include <ouisync/subscriptions.hpp>
#include <boost/asio/post.hpp>

namespace ouisync {

namespace asio = boost::asio;

// Return message ID for sending a new subscription request, if needed.
std::optional<MessageId>
Subscriptions::subscribe(
        RepositoryHandle repo_handle,
        SubscriberId subscriber_id,
        std::function<HandlerSig> handler,
        RawMessageId& next_message_id
) {
    auto by_repo_i = by_repo_handle.find(repo_handle.value);

    if (by_repo_i != by_repo_handle.end()) {
        // This is not the first subscription, so just attach to the same
        // subsciption message.
        RawMessageId message_id = by_repo_i->second;
        by_message_id[message_id].insert(subscriber_id);
        by_subscriber_id[subscriber_id] = std::make_pair(message_id, std::move(handler));
        return {};
    }

    auto message_id = next_message_id++;

    by_message_id[message_id].insert(subscriber_id);
    by_subscriber_id[subscriber_id] = std::make_pair(message_id, std::move(handler));
    by_repo_handle[repo_handle.value] = message_id;

    return MessageId{message_id};
}

template<class K, class V>
inline
std::unordered_map<K, V>::iterator
expect_find(std::unordered_map<K, V>& map, const K& key) {
    auto i = map.find(key);
    if (i == map.end()) {
        throw_error(error::logic, "key not found");
    }
    return i;
}

// Return message ID for sending an unsubscribe request, if needed.
std::optional<MessageId>
Subscriptions::unsubscribe(
    RepositoryHandle repo_handle,
    SubscriberId subscriber_id
) {
    auto by_repo_i = expect_find(by_repo_handle, repo_handle.value);
    auto message_id = by_repo_i->second;
    auto by_message_i = expect_find(by_message_id, message_id);

    by_subscriber_id.erase(subscriber_id);
    by_message_i->second.erase(subscriber_id);

    if (by_message_i->second.empty()) {
        // Last subscription unsubscribed.
        by_message_id.erase(by_message_i);
        by_repo_handle.erase(by_repo_i);
        return MessageId{message_id};
    }

    return {};
}

void
Subscriptions::handle(
    asio::any_io_executor exec,
    MessageId msg_id,
    const HandlerResult& rs
) {
    auto sub_i = by_message_id.find(msg_id.value);
    if (sub_i == by_message_id.end()) {
        return;
    }
    auto& subscriber_ids = sub_i->second;
    for (auto subscriber_id : subscriber_ids) {
        auto subscriber_i = by_subscriber_id.find(subscriber_id);

        if (subscriber_i == by_subscriber_id.end()) {
            throw_error(error::logic, "broken [SUB_TABLE] invariant");
        }

        auto handler = subscriber_i->second.second;

        asio::post(exec, [
            handler = std::move(handler),
            rs = rs
        ] () mutable {
            handler(std::move(rs));
        });
    }
}

void Subscriptions::handle_all(asio::any_io_executor exec, std::exception_ptr eptr) {
    for (const auto& [message_id, _] : by_message_id) {
        handle(exec, MessageId{message_id}, eptr);
    }
}

bool Subscriptions::is_subscribed(SubscriberId subscriber_id) {
    return by_subscriber_id.find(subscriber_id) != by_subscriber_id.end();
}

} // namespace ouisync
