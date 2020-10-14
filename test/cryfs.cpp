#include <boost/filesystem.hpp>
#include <iostream>

#include <block_store.h>
#include <create_cry_device.h>
#include <version_vector.h>

#include <fspp/fs_interface/Device.h>
#include <fspp/fs_interface/Dir.h>

using namespace ouisync;
namespace sys = boost::system;
using namespace std;

struct TestDevice {

    void ls(const fs::path& dir_path) {
        auto dir = _dev->LoadDir(dir_path);
        assert(dir);
        for (auto& ch : (*dir)->children()) {
            if (ch.name == "." || ch.name == "..") continue;
            cout << ch.name << "\n";
        }
    }

    void mkdir(const fs::path& dir_path) {
        auto is_in = [] (const auto& what, const auto& where) {
            for (auto& w : where) if (w.name == what) return true;
            return false;
        };

        fs::path p = "/";
        auto d = _dev->LoadDir(p);
        auto chs = (*d)->children();
        assert(d);
        for (auto f : dir_path) {
            if (f == "/") continue;
            p = p / f;
            if (is_in(f, chs)) {
                d = _dev->LoadDir(p);
            } else {
                (*d)->createDir(f.string(), fspp::mode_t(0),fspp::uid_t(0),fspp::gid_t(0));
                d = _dev->LoadDir(p);
            }
            assert(d);
            chs = (*d)->children();
        }
    }

    unique_ptr<fspp::Device> _dev;
};

int main() {
    fs::path testdir = fs::unique_path("/tmp/ouisync/test-cryfs-%%%%-%%%%-%%%%-%%%%");
    cout << "Testdir: " << testdir << "\n";

    {
        auto cry_device = ouisync::create_cry_device(testdir, true);
        TestDevice device{move(cry_device)};

        device.mkdir("/a/b");
        device.ls("/");
        device.ls("/a");
        sleep(2);
    }

    //cout << "Deleting " << testdir << "\n";
    //fs::remove_all(testdir);
}
