#define BOOST_TEST_MODULE dht
#include <boost/test/included/unit_test.hpp>

#include <fstream>
#include <optional>
#include <boost/asio/experimental/channel_error.hpp>
#include <boost/filesystem/operations.hpp>
#include <boost/asio.hpp>
#include <boost/asio/spawn.hpp>

#include <ouisync.hpp>
#include <ouisync/service.hpp>

#include "tests/test_utils.hpp"

namespace asio = boost::asio;
namespace fs = boost::filesystem;

struct Peer {
    ouisync::Service service;
    ouisync::Session session;

    static Peer create(const fs::path& config_dir, asio::yield_context yield) {
        mkdir(config_dir);

        ouisync::Service service(yield.get_executor());
        service.start(config_dir, config_dir.filename().c_str(), yield);
        auto session = ouisync::Session::connect(config_dir, yield);

        // Don't connect to the Mainline
        session.set_dht_routers({}, yield);
        session.set_local_dht_enabled(true, yield);
        session.bind_network({"quic/127.0.0.1:0"}, yield);

      return Peer {
          std::move(service),
          std::move(session)
      };
    }

    static Peer create_with_dht_contacts(
        const fs::path& config_dir,
        const std::vector<std::string>& dht_contacts,
        asio::yield_context yield
    )
    {
        mkdir(config_dir);

        auto dht_contacts_file = std::ofstream(config_dir / "dht_contacts_v4.txt");
        for (auto contact : dht_contacts) {
            dht_contacts_file << contact << "\n";
        }
        dht_contacts_file.close();

        return create(config_dir, yield);
    }
};

BOOST_AUTO_TEST_CASE(test_dht) {
    asio::io_context ctx;

    auto tempdir = TempDir();

    asio::spawn(ctx, [&] (asio::yield_context yield) {
        ouisync::init_log();

        auto bootstrap = Peer::create(tempdir.path() / "bootstrap", yield);
        // strip the 'quic/' prefix to obtain plain socket address.
        auto bootstrap_addr = bootstrap.session.get_local_listener_addrs(yield)[0].substr(5);

        // Pin the DHT to ensure it's running
        bootstrap.session.pin_dht(yield);

        auto alice = Peer::create_with_dht_contacts(
            tempdir.path() / "alice",
            {bootstrap_addr},
            yield
        );

        auto bob = Peer::create_with_dht_contacts(
            tempdir.path() / "bob",
            {bootstrap_addr},
            yield
        );

        auto infohash = "0000000000000000000000000000000000000b0b";

        auto bob_addr = bob.session.get_local_listener_addrs(yield)[0];

        // announce bob
        auto bob_lookup = bob.session.dht_lookup(infohash, true);

        // Wait until bob's announce completes. There are no other peers with the same infohash so
        // the channel should close without producing any.
        BOOST_REQUIRE_EXCEPTION(
            bob_lookup.async_receive(yield),
            boost::system::system_error,
            [](auto e) { return e.code() == asio::experimental::error::channel_closed; }
        );

        // alice should find bob on the DHT
        auto alice_lookup = alice.session.dht_lookup(infohash, false);
        BOOST_REQUIRE_EQUAL(alice_lookup.async_receive(yield), bob_addr);

        // bob is the only peer so the channel should now close.
        BOOST_REQUIRE_EXCEPTION(
            alice_lookup.async_receive(yield),
            boost::system::system_error,
            [](auto e) { return e.code() ==  asio::experimental::error::channel_closed; }
        );

        bootstrap.service.stop(yield);
        alice.service.stop(yield);
        bob.service.stop(yield);

        // TODO: test
    }, check_exception);

    ctx.run();
}
