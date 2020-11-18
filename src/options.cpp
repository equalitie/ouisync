#include "options.h"

#include <boost/asio/ip/address.hpp>
#include <iostream>

using namespace ouisync;
using namespace boost::program_options;
using net::ip::tcp;

namespace std {
    // XXX: Move this somewhere else
    static std::istream& operator>>(std::istream& is, tcp::endpoint& ep)
    {
        using I = std::istreambuf_iterator<char>;

        std::string addr_str;

        for (auto i = I(is); i != I(); ++i)
        {
            if (*i == ':') { break; }
            addr_str.push_back(*i);
        }

        char colon = '\0';
        is >> colon;

        if (colon != ':')
            throw std::runtime_error("Failed to parse endpoint");

        auto addr = net::ip::make_address(addr_str);

        uint16_t port = -1;
        is >> port;

        ep = tcp::endpoint(addr, port);

        return is;
    }
}

Options::Options() :
    help(false),
    description("Options")
{
    description.add_options()
        ( "help,h", value(&help), "produce this help")

        ( "basedir", value(&basedir)->required(), "base directory")

        ( "mountdir", value(&mountdir), "mount directory")

        ( "connect", value(&connect_endpoint), "peer's TCP endpoint")

        ( "accept", value(&accept_endpoint), "our accepting TCP endpoint")

        ;
}

void Options::parse(unsigned argc, char** argv)
{
    variables_map vars;
    store(parse_command_line(argc, argv, description), vars);
    notify(vars);

    branchdir         = basedir / "branches";
    objectdir         = basedir / "objects";
    snapshotdir       = basedir / "snapshots";
    remotes           = basedir / "remotes";
    user_id_file_path = basedir / "user_id";
}

void Options::write_help(std::ostream& os)
{
    os << description;
}
