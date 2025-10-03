#define BOOST_TEST_MODULE Subscriptions
#include <boost/test/included/unit_test.hpp>

// Gain access to private members
#define OUISYNC_TESTING

#include <ouisync/subscriptions.hpp>
#include "test_utils.hpp"

using namespace ouisync;

namespace std {
    ostream& operator<<(ostream& os, const Subscriptions& subs) {
        return os
            << "MessageId -> Set<SubscriberId>\n"
            << "  " << subs.by_message_id << "\n"
            << "SubscriberId -> (MessageId, Handler)\n"
            << "  " << subs.by_subscriber_id << "\n"
            << "RepoHandle -> MessageId\n"
            << "  " << subs.by_repo_handle;
    }
    template<class Sig>
    ostream& operator<<(ostream& os, const std::function<Sig>&) {
        return os << "fn(...)";
    }
} // namespace std

using RawMessageId = Subscriptions::RawMessageId;

BOOST_AUTO_TEST_CASE(test_subscribe_unsubscribe) {
    Subscriptions subs;

    RawMessageId next_message_id = 0;
    auto repo_handle = RepositoryHandle{0};
    SubscriberId subscriber_id = 0;
    uint64_t notify_counter = 0;

    auto rq_msg_id = subs.subscribe(
            repo_handle,
            subscriber_id,
            [&] (HandlerResult) { notify_counter++; },
            next_message_id
        );
    
    BOOST_TEST(rq_msg_id == MessageId{0});
    BOOST_TEST(notify_counter == 0);
    BOOST_TEST(subs.is_subscribed(subscriber_id));

    auto rs_msg_id = subs.unsubscribe(repo_handle, subscriber_id);

    BOOST_TEST(rs_msg_id == MessageId{0});
    BOOST_TEST(notify_counter == 0);
    BOOST_TEST(!subs.is_subscribed(subscriber_id));

    BOOST_TEST(subs.by_message_id.empty());
    BOOST_TEST(subs.by_subscriber_id.empty());
    BOOST_TEST(subs.by_repo_handle.empty());
}

BOOST_AUTO_TEST_CASE(test_subscribe_unsubscribe_2x_same_repo) {
    Subscriptions subs;

    RawMessageId next_message_id = 0;
    auto repo_handle = RepositoryHandle{0};
    SubscriberId subscriber1_id = 0;
    SubscriberId subscriber2_id = 1;
    uint64_t notify_counter = 0;

    auto rq1_msg_id = subs.subscribe(
            repo_handle,
            subscriber1_id,
            [&] (HandlerResult) { notify_counter++; },
            next_message_id
        );
    
    BOOST_TEST(rq1_msg_id == MessageId{0});
    BOOST_TEST(notify_counter == 0);
    BOOST_TEST(subs.is_subscribed(subscriber1_id));

    auto rq2_msg_id = subs.subscribe(
            repo_handle,
            subscriber2_id,
            [&] (HandlerResult) { notify_counter++; },
            next_message_id
        );
    
    BOOST_TEST(rq2_msg_id == std::optional<MessageId>());
    BOOST_TEST(notify_counter == 0);
    BOOST_TEST(subs.is_subscribed(subscriber2_id));

    auto rs1_msg_id = subs.unsubscribe(repo_handle, subscriber1_id);

    BOOST_TEST(rs1_msg_id == std::optional<MessageId>());
    BOOST_TEST(notify_counter == 0);
    BOOST_TEST(!subs.is_subscribed(subscriber1_id));
    BOOST_TEST(subs.is_subscribed(subscriber2_id));

    BOOST_TEST(!subs.by_message_id.empty());
    BOOST_TEST(!subs.by_subscriber_id.empty());
    BOOST_TEST(!subs.by_repo_handle.empty());

    auto rs2_msg_id = subs.unsubscribe(repo_handle, subscriber2_id);

    BOOST_TEST(rs2_msg_id == MessageId{0});
    BOOST_TEST(notify_counter == 0);
    BOOST_TEST(!subs.is_subscribed(subscriber2_id));

    BOOST_TEST(subs.by_message_id.empty());
    BOOST_TEST(subs.by_subscriber_id.empty());
    BOOST_TEST(subs.by_repo_handle.empty());
}
