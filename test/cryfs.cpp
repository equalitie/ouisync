#include <boost/filesystem.hpp>
#include <iostream>

#include <block_store.h>
#include <create_cry_device.h>
#include <defer.h>

#include <cryfs/impl/filesystem/CryDevice.h>

using namespace ouisync;
namespace sys = boost::system;
using namespace std;
template<class T> using Ref = cpputils::unique_ref<T>;

void ls(cryfs::CryDevice& dev, const fs::path& path) {
    auto dir = dev.LoadDir(path);
    assert(dir);
    for (auto& ch : (*dir)->children()) {
        if (ch.name == "." || ch.name == "..") continue;
        cout << ch.name << "\n";
    }
}

int main() {
    fs::path testdir = fs::unique_path("/tmp/ouisync-test-%%%%-%%%%-%%%%-%%%%");

    cout << "Testdir: " << testdir << "\n";

    auto device = ouisync::create_cry_device(testdir);

    ls(*device, "/");

    device = nullptr;

    cout << "Deleting " << testdir << "\n";
    fs::remove_all(testdir);
}
