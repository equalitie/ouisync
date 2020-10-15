#define BOOST_TEST_MODULE objects
#include <boost/test/included/unit_test.hpp>

#include <boost/filesystem.hpp>
#include <iostream>

#include <block_store.h>
#include <create_cry_device.h>

#include <fspp/fs_interface/Device.h>
#include <fspp/fs_interface/File.h>
#include <fspp/fs_interface/Dir.h>
#include <fspp/fs_interface/OpenFile.h>

using namespace ouisync;
namespace sys = boost::system;
using namespace std;

template<class T> using Ref = cpputils::unique_ref<T>;

using Dir      = fspp::Dir;
using File     = fspp::File;
using OpenFile = fspp::OpenFile;

struct TestDevice {
    void ls(const fs::path& dir_path) {
        auto dir = _dev->LoadDir(dir_path);
        assert(dir);
        for (auto& ch : (*dir)->children()) {
            if (ch.name == "." || ch.name == "..") continue;
            cout << ch.name << "\n";
        }
    }

    Ref<Dir> mkdir(const fs::path& dir_path) {
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
                (*d)->createDir(f.string(),
                        fspp::mode_t(0), fspp::uid_t(0), fspp::gid_t(0));
                d = _dev->LoadDir(p);
            }
            assert(d);
            chs = (*d)->children();
        }
        return std::move(*d);
    }

    Ref<OpenFile> mkfile(const fs::path& path) {
        auto dir = mkdir(path.parent_path());
        return dir->createAndOpenFile(path.filename().native(),
                fspp::mode_t(0), fspp::uid_t(0), fspp::gid_t(0));
    }

    Ref<OpenFile> open_file(const fs::path& path) {
        Opt<Ref<File>> file = _dev->LoadFile(path);
        assert(file);
        return (*file)->open(fspp::openflags_t::RDONLY());
    }

    template<class T, class Ts>
    bool is_in(const T& what, const Ts& where) const {
        for (auto& w : where) if (w.name == what) return true;
        return false;
    };

    unique_ptr<fspp::Device> _dev;
};

void write(Ref<OpenFile>& file, const std::vector<char>& v) {
    file->write(v.data(), fspp::num_bytes_t(v.size()), fspp::num_bytes_t(0));
}

std::vector<char> read(Ref<OpenFile>& file, size_t size) {
    std::vector<char> v(size);
    auto s = file->read(v.data(), fspp::num_bytes_t(size), fspp::num_bytes_t(0));
    v.resize(s.value());
    return v;
}

namespace std {
    std::ostream& operator<<(std::ostream& os, const std::vector<char>& v) {
        for (auto c : v) os << int(c);
        return os;
    }
}

BOOST_AUTO_TEST_CASE(store_and_restore) {
    fs::path testdir = fs::unique_path("/tmp/ouisync/test-cryfs-%%%%-%%%%-%%%%-%%%%");
    cout << "Testdir: " << testdir << "\n";

    {
        auto cry_device = ouisync::create_cry_device(testdir, true);
        TestDevice device{move(cry_device)};

        auto f = device.mkfile("/a/b");
        write(f, {1,2,3});
        f->flush();
    }

    {
        auto cry_device = ouisync::create_cry_device(testdir, true);
        TestDevice device{move(cry_device)};
        auto f = device.open_file("/a/b");
        auto v = read(f, 3);
        BOOST_REQUIRE_EQUAL(v, decltype(v)({1,2,3}));
    }
}
