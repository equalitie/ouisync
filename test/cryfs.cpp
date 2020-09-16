#include <boost/filesystem.hpp>
#include <iostream>

#include <block_store.h>
#include <create_cry_device.h>

#include <cryfs/impl/filesystem/CryDevice.h>

using namespace ouisync;
namespace sys = boost::system;
using namespace std;

int main() {
    fs::path testdir = fs::unique_path("/tmp/ouisync-test-%%%%-%%%%-%%%%-%%%%");
    cerr << "Testdir: " << testdir << "\n";

    auto device = ouisync::create_cry_device(testdir);

    auto dir = device->LoadDir("/");

    assert(dir);

    auto children = (*dir)->children();
    cerr << "children count: " << children.size() << "\n";
    for (auto& c : children) {
        cerr << "c: " << c.name << "\n";
    }
}
