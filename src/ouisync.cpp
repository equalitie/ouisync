#include "user_id.h"
#include "branch.h"
#include "fuse.h"
#include "shortcuts.h"

#include <boost/asio.hpp>
#include <boost/filesystem.hpp>

#include <iostream>

using namespace std;
using namespace ouisync;

int main() {
    net::io_context ioc;

    auto basedir = fs::unique_path("/tmp/ouisync/test-%%%%-%%%%-%%%%-%%%%");

    cout << "Basedir: " << basedir << "\n";

    auto mountdir = basedir / "mountdir";
    auto branchdir = basedir / "branches";
    auto objdir = basedir / "objects";

    fs::create_directories(mountdir);
    fs::create_directories(branchdir);
    fs::create_directories(objdir);

    auto user_id = UserId::load_or_create(basedir / "user_id");
    auto branch = Branch::load_or_create(branchdir, objdir, user_id);

    Fuse fuse(ioc.get_executor(), mountdir);

    net::signal_set signals(ioc, SIGINT, SIGTERM);
    signals.async_wait([&] (sys::error_code, int) {
        fuse.finish();
        ioc.reset();
    });

    ioc.run();
}
