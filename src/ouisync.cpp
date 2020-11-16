#include "user_id.h"
#include "branch.h"
#include "fuse_runner.h"
#include "file_system.h"
#include "network.h"
#include "shortcuts.h"
#include "options.h"

#include <boost/asio.hpp>
#include <boost/filesystem.hpp>

#include <iostream>

using namespace std;
using namespace ouisync;

int main(int argc, char* argv[]) {
    net::io_context ioc;

    Options options;

    try {
        options.parse(argc, argv);

        if (options.help) {
            options.write_help(cout);
            exit(0);
        }
    }
    catch (const std::exception& e) {
        cerr << "Failed to parse options:\n";
        cerr << e.what() << "\n\n";
        options.write_help(cerr);
        exit(1);
    }

    fs::create_directories(options.branchdir);
    fs::create_directories(options.objectdir);
    fs::create_directories(options.mountdir);
    fs::create_directories(options.snapshotdir);

    FileSystem fs(ioc.get_executor(), options);
    Network network(ioc.get_executor(), fs, options);
    FuseRunner fuse(fs, options.mountdir);

    net::signal_set signals(ioc, SIGINT, SIGTERM, SIGHUP);
    signals.async_wait([&] (sys::error_code, int) {
        fuse.finish();
        network.finish();
    });

    ioc.run();
}
