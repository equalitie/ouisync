#include <boost/filesystem.hpp>
#include <iostream>

#include <cryfs/impl/localstate/LocalStateDir.h>
#include <cryfs/impl/config/CryConfigLoader.h>
#include <cryfs/impl/filesystem/CryDevice.h>
#include <blockstore/implementations/ondisk/OnDiskBlockStore2.h>
#include <cpp-utils/io/IOStreamConsole.h>
#include <cpp-utils/io/NoninteractiveConsole.h>
#include <cryfs/impl/config/CryPasswordBasedKeyProvider.h>
#include <cpp-utils/random/OSRandomGenerator.h>

namespace sys = boost::system;
using namespace std;
namespace fs = boost::filesystem;

cpputils::either<cryfs::CryConfigFile::LoadError, cryfs::CryConfigLoader::ConfigLoadResult>
loadOrCreateConfigFile(fs::path config_file, cryfs::LocalStateDir statedir) {
    auto console_ = make_shared<cpputils::IOStreamConsole>();
    auto console  = make_shared<cpputils::NoninteractiveConsole>(move(console_));

    boost::optional<uint32_t> block_size_bytes;
    bool allow_filesystem_upgrade = false;
    bool allow_replaced_filesystem = false;
    boost::optional<bool> missing_block_is_integrity_violation;
    boost::optional<string> cipher;

    auto &key_generator = cpputils::Random::OSRandom();
    auto settings = cpputils::SCrypt::DefaultSettings;

    auto askPassword = [] { return "test-password"; };

    auto keyProvider = cpputils::make_unique_ref<cryfs::CryPasswordBasedKeyProvider>(
      console,
      askPassword,
      askPassword,
      cpputils::make_unique_ref<cpputils::SCrypt>(settings)
    );

    return cryfs::CryConfigLoader(console,
            key_generator,
            std::move(keyProvider),
            std::move(statedir),
            cipher,
            block_size_bytes,
            missing_block_is_integrity_violation
            )
        .loadOrCreate(std::move(config_file), allow_filesystem_upgrade, allow_replaced_filesystem);
}

cryfs::CryConfigLoader::ConfigLoadResult
loadOrCreateConfig(const fs::path& config_file, const cryfs::LocalStateDir& statedir) {
    auto config = loadOrCreateConfigFile(config_file, statedir);

    if (config.is_left()) {
        switch(config.left()) {
            case cryfs::CryConfigFile::LoadError::DecryptionFailed:
                cerr << "Failed to decrypt the config file. Did you enter the correct password?\n";
            case cryfs::CryConfigFile::LoadError::ConfigFileNotFound:
                cerr << "Could not find the cryfs.config file. Are you sure this is a valid CryFS file system?\n";
        }
        exit(1);
    }

    // XXX
    //_checkConfigIntegrity(options.baseDir(), statedir, *config.right().configFile, options.allowReplacedFilesystem());

    return std::move(config.right());
}

class SyncBlockStore : public blockstore::BlockStore2 {
public:
    using BlockId = blockstore::BlockId;

public:
    SyncBlockStore(fs::path basedir)
        : _bs(std::move(basedir))
    {}

    BlockId createBlockId() const override {
        auto r = _bs.createBlockId();
        cerr << "createBlockId() -> " << r.ToString() << "\n";
        return r;
    }

    WARN_UNUSED_RESULT
    bool tryCreate(const BlockId &blockId, const cpputils::Data &data) override {
        bool b = _bs.tryCreate(blockId, data);
        cerr << "tryCreate(" << blockId.ToString() << ") -> " << b << "\n";
        return b;
    }

    WARN_UNUSED_RESULT
    bool remove(const BlockId &blockId) override {
        auto b = _bs.remove(blockId);
        cerr << "remove(" << blockId.ToString() << ") -> " << b << "\n";
        return b;
    }

    WARN_UNUSED_RESULT
    boost::optional<cpputils::Data> load(const BlockId &blockId) const override {
        cerr << "load(" << blockId.ToString() << ")\n";
        return _bs.load(blockId);
    }

    // Store the block with the given blockId. If it doesn't exist, it is created.
    void store(const BlockId &blockId, const cpputils::Data &data) {
        cerr << "store(" << blockId.ToString() << ")\n";
        return _bs.store(blockId, data);
    }

    uint64_t numBlocks() const override {
        auto r = _bs.numBlocks();
        cerr << "numBlocks() -> " << r << "\n";
        return r;
    }

    uint64_t estimateNumFreeBytes() const override {
        auto r = _bs.estimateNumFreeBytes();
        cerr << "estimateNumFreeBytes() -> " << r << "\n";
        return r;
    }

    uint64_t blockSizeFromPhysicalBlockSize(uint64_t blockSize) const override {
        auto r = _bs.blockSizeFromPhysicalBlockSize(blockSize);
        cerr << "blockSizeFromPhysicalBlockSize(" << blockSize << ") -> " << r  << "\n";
        return r;
    }

    void forEachBlock(std::function<void (const BlockId &)> callback) const override {
        cerr << "forEachBlock\n";
        _bs.forEachBlock(std::move(callback));
    }

    virtual ~SyncBlockStore() {}

private:
    blockstore::ondisk::OnDiskBlockStore2 _bs;
};

int main() {
    fs::path testdir = fs::unique_path("/tmp/ouisync-test-%%%%-%%%%-%%%%-%%%%");
    fs::create_directories(testdir);

    cerr << "Testdir: " << testdir << "\n";

    fs::path basedir = testdir / "basedir";

    fs::create_directories(basedir);

    const int argc = 1;
    const char* argv[] = {"test"};

    fs::path config_file = basedir / "cryfs.config";

    cryfs::LocalStateDir statedir(testdir / "statedir");

    auto blockStore = cpputils::make_unique_ref<SyncBlockStore>(basedir);

    cryfs::CryConfigLoader::ConfigLoadResult config = loadOrCreateConfig(config_file, statedir);

    auto onIntegrityViolation = [] () {
        cerr << "Integrity has been violated\n";
    };

    bool allowIntegrityViolations = false;
    const bool missingBlockIsIntegrityViolation = true;

    cryfs::CryDevice device(
            std::move(config.configFile),
            std::move(blockStore),
            statedir,
            config.myClientId,
            allowIntegrityViolations,
            missingBlockIsIntegrityViolation,
            std::move(onIntegrityViolation));
}
