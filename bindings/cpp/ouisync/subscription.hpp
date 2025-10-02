#include <boost/asio/spawn.hpp>

namespace ouisync {

class Repository;

class RepositorySubscription {
public:
    void subscribe(Repository&, boost::asio::yield_context);
    void unsubscribe(boost::asio::yield_context);

    //! Wait for a change in the repository we subscribed to.
    void state_changed(boost::asio::yield_context);

    RepositorySubscription() = default;
    RepositorySubscription(RepositorySubscription&&) = default;
    RepositorySubscription(RepositorySubscription const&) = delete;
    RepositorySubscription& operator=(RepositorySubscription const&) = delete;

    //! Calls `unsubscribe` in detached mode
    ~RepositorySubscription();

private:
    struct Impl;
    std::shared_ptr<Impl> _impl;
};

} // namespace ouisync
