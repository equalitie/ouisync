#include <create_cry_device.h>
#include <block_store.h>

#include <cryfs/impl/localstate/LocalStateDir.h>
#include <cryfs/impl/config/CryConfigLoader.h>
#include <cryfs/impl/filesystem/CryDevice.h>
#include <cpp-utils/io/IOStreamConsole.h>
#include <cpp-utils/io/NoninteractiveConsole.h>
#include <cryfs/impl/config/CryPasswordBasedKeyProvider.h>
#include <cryfs/impl/config/CryPresetPasswordBasedKeyProvider.h>
#include <cpp-utils/random/OSRandomGenerator.h>

using namespace ouisync;
namespace sys = boost::system;
using namespace std;

inline
cpputils::unique_ref<cryfs::CryKeyProvider>
create_key_provider(shared_ptr<cpputils::NoninteractiveConsole> console) {
    auto askPassword = [] { return "test-password"; };
    auto settings = cpputils::SCrypt::DefaultSettings;
    return cpputils::make_unique_ref<cryfs::CryPasswordBasedKeyProvider>(
      console,
      askPassword,
      askPassword,
      cpputils::make_unique_ref<cpputils::SCrypt>(settings)
    );
}

inline
cpputils::unique_ref<cryfs::CryKeyProvider>
create_test_key_provider() {
    auto settings = cpputils::SCrypt::TestSettings;
    return cpputils::make_unique_ref<cryfs::CryPresetPasswordBasedKeyProvider>(
        "mypassword", cpputils::make_unique_ref<cpputils::SCrypt>(settings)
    );
}

static
cpputils::either<cryfs::CryConfigFile::LoadError, cryfs::CryConfigLoader::ConfigLoadResult>
loadOrCreateConfigFile(fs::path config_file, cryfs::LocalStateDir statedir, bool test) {
    auto console_ = make_shared<cpputils::IOStreamConsole>();
    auto console  = make_shared<cpputils::NoninteractiveConsole>(move(console_));

    boost::optional<uint32_t> block_size_bytes;
    bool allow_filesystem_upgrade = false;
    bool allow_replaced_filesystem = false;
    boost::optional<bool> missing_block_is_integrity_violation;
    boost::optional<string> cipher;


    auto &key_generator = cpputils::Random::OSRandom();

    return cryfs::CryConfigLoader(console,
            key_generator,
            test ? create_test_key_provider() : create_key_provider(console),
            std::move(statedir),
            cipher,
            block_size_bytes,
            missing_block_is_integrity_violation
            )
        .loadOrCreate(std::move(config_file), allow_filesystem_upgrade, allow_replaced_filesystem);
}

static
cryfs::CryConfigLoader::ConfigLoadResult
loadOrCreateConfig(const fs::path& config_file, const cryfs::LocalStateDir& statedir, bool test) {
    auto config = loadOrCreateConfigFile(config_file, statedir, test);

    if (config.is_left()) {
        switch(config.left()) {
            case cryfs::CryConfigFile::LoadError::DecryptionFailed:
                cerr << "Failed to decrypt the config file. Did you enter the correct password?\n";
                break;
            case cryfs::CryConfigFile::LoadError::ConfigFileNotFound:
                cerr << "Could not find the cryfs.config file. Are you sure this is a valid CryFS file system?\n";
                break;
        }
        exit(1);
    }

    // XXX
    //_checkConfigIntegrity(options.baseDir(), statedir, *config.right().configFile, options.allowReplacedFilesystem());

    return std::move(config.right());
}

namespace ouisync {

unique_ptr<fspp::Device>
create_cry_device(const fs::path& rootdir, bool test)
{
    fs::path basedir(rootdir / "basedir");
    fs::create_directories(basedir);
    fs::path config_file = basedir / "cryfs.config";
    cryfs::LocalStateDir statedir(rootdir / "statedir");

    auto blockStore = cpputils::make_unique_ref<ouisync::BlockStore>(basedir);

    cryfs::CryConfigLoader::ConfigLoadResult config = loadOrCreateConfig(config_file, statedir, test);

    auto onIntegrityViolation = [] () {
        cerr << "Integrity has been violated\n";
    };

    bool allowIntegrityViolations = false;
    const bool missingBlockIsIntegrityViolation = true;

    auto dev = make_unique<cryfs::CryDevice>(
            std::move(config.configFile),
            std::move(blockStore),
            statedir,
            config.myClientId,
            allowIntegrityViolations,
            missingBlockIsIntegrityViolation,
            std::move(onIntegrityViolation));

    dev->setContext(fspp::Context(fspp::noatime()));

    return dev;
}

} // ouisync namespace
