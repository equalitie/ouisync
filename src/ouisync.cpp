#include "user_id.h"
#include "branch.h"
#include "fuse_runner.h"
#include "file_system.h"
#include "shortcuts.h"
#include "file_system_options.h"

#include <boost/asio.hpp>
#include <boost/filesystem.hpp>

#include <iostream>

using namespace std;
using namespace ouisync;

int main() {
    net::io_context ioc;

    auto basedir = fs::unique_path("/tmp/ouisync/test-%%%%-%%%%-%%%%-%%%%");

    cout << "Basedir: " << basedir << "\n";

    FileSystemOptions opts(basedir);

    auto mountdir = basedir / "mountdir";

    fs::create_directories(mountdir);

    FileSystem fs(ioc.get_executor(), move(opts));
    FuseRunner fuse(fs, mountdir);

    net::signal_set signals(ioc, SIGINT, SIGTERM);
    signals.async_wait([&] (sys::error_code, int) {
        fuse.finish();
        ioc.reset();
    });

    ioc.run();
}
