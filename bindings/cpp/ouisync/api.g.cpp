// This file is auto generated. Do not edit.

#include <ouisync/client.hpp>
#include <ouisync/api.g.hpp>
#include <ouisync/message.g.hpp>

namespace ouisync {

void File::close(
    boost::asio::yield_context yield
) {
    auto request = Request::FileClose{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void File::flush(
    boost::asio::yield_context yield
) {
    auto request = Request::FileFlush{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

uint64_t File::get_length(
    boost::asio::yield_context yield
) {
    auto request = Request::FileGetLength{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::U64 rsp = std::move(response.get<Response::U64>());
    return rsp.value;
}

uint64_t File::get_progress(
    boost::asio::yield_context yield
) {
    auto request = Request::FileGetProgress{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::U64 rsp = std::move(response.get<Response::U64>());
    return rsp.value;
}

std::vector<uint8_t> File::read(
    uint64_t offset,
    uint64_t size,
    boost::asio::yield_context yield
) {
    auto request = Request::FileRead{
        handle,
        offset,
        size,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Bytes rsp = std::move(response.get<Response::Bytes>());
    return rsp.value;
}

void File::truncate(
    uint64_t len,
    boost::asio::yield_context yield
) {
    auto request = Request::FileTruncate{
        handle,
        len,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void File::write(
    uint64_t offset,
    const std::vector<uint8_t>& data,
    boost::asio::yield_context yield
) {
    auto request = Request::FileWrite{
        handle,
        offset,
        data,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::close(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryClose{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::create_directory(
    const std::string& path,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryCreateDirectory{
        handle,
        path,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

File Repository::create_file(
    const std::string& path,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryCreateFile{
        handle,
        path,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::File rsp = std::move(response.get<Response::File>());
    return File(client, std::move(rsp.value));
}

void Repository::create_mirror(
    const std::string& host,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryCreateMirror{
        handle,
        host,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::delete_repository(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryDelete{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::delete_mirror(
    const std::string& host,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryDeleteMirror{
        handle,
        host,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

std::string Repository::export_repository(
    const std::string& output_path,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryExport{
        handle,
        output_path,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Path rsp = std::move(response.get<Response::Path>());
    return rsp.value;
}

bool Repository::file_exists(
    const std::string& path,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryFileExists{
        handle,
        path,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Bool rsp = std::move(response.get<Response::Bool>());
    return rsp.value;
}

AccessMode Repository::get_access_mode(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryGetAccessMode{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::AccessMode rsp = std::move(response.get<Response::AccessMode>());
    return rsp.value;
}

std::optional<std::chrono::milliseconds> Repository::get_block_expiration(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryGetBlockExpiration{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::Duration>() == nullptr) return {};
    return std::move(response.get<Response::Duration>()).value;
}

std::vector<uint8_t> Repository::get_credentials(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryGetCredentials{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Bytes rsp = std::move(response.get<Response::Bytes>());
    return rsp.value;
}

std::optional<EntryType> Repository::get_entry_type(
    const std::string& path,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryGetEntryType{
        handle,
        path,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::EntryType>() == nullptr) return {};
    return std::move(response.get<Response::EntryType>()).value;
}

std::optional<std::chrono::milliseconds> Repository::get_expiration(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryGetExpiration{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::Duration>() == nullptr) return {};
    return std::move(response.get<Response::Duration>()).value;
}

std::string Repository::get_info_hash(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryGetInfoHash{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::String rsp = std::move(response.get<Response::String>());
    return rsp.value;
}

std::optional<std::string> Repository::get_metadata(
    const std::string& key,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryGetMetadata{
        handle,
        key,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::String>() == nullptr) return {};
    return std::move(response.get<Response::String>()).value;
}

std::optional<std::string> Repository::get_mount_point(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryGetMountPoint{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::Path>() == nullptr) return {};
    return std::move(response.get<Response::Path>()).value;
}

std::string Repository::get_path(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryGetPath{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Path rsp = std::move(response.get<Response::Path>());
    return rsp.value;
}

QuotaInfo Repository::get_quota(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryGetQuota{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::QuotaInfo rsp = std::move(response.get<Response::QuotaInfo>());
    return rsp.value;
}

Stats Repository::get_stats(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryGetStats{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Stats rsp = std::move(response.get<Response::Stats>());
    return rsp.value;
}

Progress Repository::get_sync_progress(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryGetSyncProgress{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Progress rsp = std::move(response.get<Response::Progress>());
    return rsp.value;
}

bool Repository::is_dht_enabled(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryIsDhtEnabled{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Bool rsp = std::move(response.get<Response::Bool>());
    return rsp.value;
}

bool Repository::is_pex_enabled(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryIsPexEnabled{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Bool rsp = std::move(response.get<Response::Bool>());
    return rsp.value;
}

bool Repository::is_sync_enabled(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryIsSyncEnabled{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Bool rsp = std::move(response.get<Response::Bool>());
    return rsp.value;
}

bool Repository::mirror_exists(
    const std::string& host,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryMirrorExists{
        handle,
        host,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Bool rsp = std::move(response.get<Response::Bool>());
    return rsp.value;
}

std::string Repository::mount(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryMount{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Path rsp = std::move(response.get<Response::Path>());
    return rsp.value;
}

void Repository::move(
    const std::string& dst,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryMove{
        handle,
        dst,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::move_entry(
    const std::string& src,
    const std::string& dst,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryMoveEntry{
        handle,
        src,
        dst,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

File Repository::open_file(
    const std::string& path,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryOpenFile{
        handle,
        path,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::File rsp = std::move(response.get<Response::File>());
    return File(client, std::move(rsp.value));
}

std::vector<DirectoryEntry> Repository::read_directory(
    const std::string& path,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryReadDirectory{
        handle,
        path,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::DirectoryEntries rsp = std::move(response.get<Response::DirectoryEntries>());
    return rsp.value;
}

void Repository::remove_directory(
    const std::string& path,
    boost::asio::yield_context yield,
    bool recursive
) {
    auto request = Request::RepositoryRemoveDirectory{
        handle,
        path,
        recursive,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::remove_file(
    const std::string& path,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryRemoveFile{
        handle,
        path,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::reset_access(
    const ShareToken& token,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryResetAccess{
        handle,
        token,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::set_access(
    const std::optional<AccessChange>& read,
    const std::optional<AccessChange>& write,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositorySetAccess{
        handle,
        read,
        write,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::set_access_mode(
    const AccessMode& access_mode,
    boost::asio::yield_context yield,
    const std::optional<LocalSecret>& local_secret
) {
    auto request = Request::RepositorySetAccessMode{
        handle,
        access_mode,
        local_secret,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::set_block_expiration(
    const std::optional<std::chrono::milliseconds>& value,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositorySetBlockExpiration{
        handle,
        value,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::set_credentials(
    const std::vector<uint8_t>& credentials,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositorySetCredentials{
        handle,
        credentials,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::set_dht_enabled(
    bool enabled,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositorySetDhtEnabled{
        handle,
        enabled,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::set_expiration(
    const std::optional<std::chrono::milliseconds>& value,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositorySetExpiration{
        handle,
        value,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

bool Repository::set_metadata(
    const std::vector<MetadataEdit>& edits,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositorySetMetadata{
        handle,
        edits,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Bool rsp = std::move(response.get<Response::Bool>());
    return rsp.value;
}

void Repository::set_pex_enabled(
    bool enabled,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositorySetPexEnabled{
        handle,
        enabled,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::set_quota(
    const std::optional<StorageSize>& value,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositorySetQuota{
        handle,
        value,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Repository::set_sync_enabled(
    bool enabled,
    boost::asio::yield_context yield
) {
    auto request = Request::RepositorySetSyncEnabled{
        handle,
        enabled,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

ShareToken Repository::share(
    const AccessMode& access_mode,
    boost::asio::yield_context yield,
    const std::optional<LocalSecret>& local_secret
) {
    auto request = Request::RepositoryShare{
        handle,
        access_mode,
        local_secret,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::ShareToken rsp = std::move(response.get<Response::ShareToken>());
    return rsp.value;
}

void Repository::unmount(
    boost::asio::yield_context yield
) {
    auto request = Request::RepositoryUnmount{
        handle,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

Session Session::connect(
    const boost::filesystem::path& config_dir,
    boost::asio::yield_context yield
) {
    auto client = ouisync::Client::connect(config_dir, yield);
    return Session(std::make_shared<Client>(std::move(client)));
}

void Session::add_user_provided_peers(
    const std::vector<std::string>& addrs,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionAddUserProvidedPeers{
        addrs,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Session::bind_metrics(
    const std::optional<std::string>& addr,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionBindMetrics{
        addr,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Session::bind_network(
    const std::vector<std::string>& addrs,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionBindNetwork{
        addrs,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

uint16_t Session::bind_remote_control(
    const std::optional<std::string>& addr,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionBindRemoteControl{
        addr,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::U16 rsp = std::move(response.get<Response::U16>());
    return rsp.value;
}

Repository Session::create_repository(
    const std::string& path,
    boost::asio::yield_context yield,
    const std::optional<SetLocalSecret>& read_secret,
    const std::optional<SetLocalSecret>& write_secret,
    const std::optional<ShareToken>& token,
    bool sync_enabled,
    bool dht_enabled,
    bool pex_enabled
) {
    auto request = Request::SessionCreateRepository{
        path,
        read_secret,
        write_secret,
        token,
        sync_enabled,
        dht_enabled,
        pex_enabled,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Repository rsp = std::move(response.get<Response::Repository>());
    return Repository(client, std::move(rsp.value));
}

void Session::delete_repository_by_name(
    const std::string& name,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionDeleteRepositoryByName{
        name,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

SecretKey Session::derive_secret_key(
    const Password& password,
    const PasswordSalt& salt,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionDeriveSecretKey{
        password,
        salt,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::SecretKey rsp = std::move(response.get<Response::SecretKey>());
    return rsp.value;
}

Repository Session::find_repository(
    const std::string& name,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionFindRepository{
        name,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Repository rsp = std::move(response.get<Response::Repository>());
    return Repository(client, std::move(rsp.value));
}

PasswordSalt Session::generate_password_salt(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGeneratePasswordSalt();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::PasswordSalt rsp = std::move(response.get<Response::PasswordSalt>());
    return rsp.value;
}

SecretKey Session::generate_secret_key(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGenerateSecretKey();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::SecretKey rsp = std::move(response.get<Response::SecretKey>());
    return rsp.value;
}

uint64_t Session::get_current_protocol_version(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetCurrentProtocolVersion();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::U64 rsp = std::move(response.get<Response::U64>());
    return rsp.value;
}

std::optional<std::chrono::milliseconds> Session::get_default_block_expiration(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetDefaultBlockExpiration();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::Duration>() == nullptr) return {};
    return std::move(response.get<Response::Duration>()).value;
}

std::optional<StorageSize> Session::get_default_quota(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetDefaultQuota();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::StorageSize>() == nullptr) return {};
    return std::move(response.get<Response::StorageSize>()).value;
}

std::optional<std::chrono::milliseconds> Session::get_default_repository_expiration(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetDefaultRepositoryExpiration();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::Duration>() == nullptr) return {};
    return std::move(response.get<Response::Duration>()).value;
}

std::optional<std::string> Session::get_external_addr_v4(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetExternalAddrV4();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::SocketAddr>() == nullptr) return {};
    return std::move(response.get<Response::SocketAddr>()).value;
}

std::optional<std::string> Session::get_external_addr_v6(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetExternalAddrV6();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::SocketAddr>() == nullptr) return {};
    return std::move(response.get<Response::SocketAddr>()).value;
}

uint64_t Session::get_highest_seen_protocol_version(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetHighestSeenProtocolVersion();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::U64 rsp = std::move(response.get<Response::U64>());
    return rsp.value;
}

std::vector<std::string> Session::get_local_listener_addrs(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetLocalListenerAddrs();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::PeerAddrs rsp = std::move(response.get<Response::PeerAddrs>());
    return rsp.value;
}

std::optional<std::string> Session::get_metrics_listener_addr(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetMetricsListenerAddr();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::SocketAddr>() == nullptr) return {};
    return std::move(response.get<Response::SocketAddr>()).value;
}

std::optional<std::string> Session::get_mount_root(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetMountRoot();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::Path>() == nullptr) return {};
    return std::move(response.get<Response::Path>()).value;
}

std::optional<NatBehavior> Session::get_nat_behavior(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetNatBehavior();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::NatBehavior>() == nullptr) return {};
    return std::move(response.get<Response::NatBehavior>()).value;
}

Stats Session::get_network_stats(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetNetworkStats();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Stats rsp = std::move(response.get<Response::Stats>());
    return rsp.value;
}

std::vector<PeerInfo> Session::get_peers(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetPeers();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::PeerInfos rsp = std::move(response.get<Response::PeerInfos>());
    return rsp.value;
}

std::optional<std::string> Session::get_remote_control_listener_addr(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetRemoteControlListenerAddr();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::SocketAddr>() == nullptr) return {};
    return std::move(response.get<Response::SocketAddr>()).value;
}

std::vector<std::string> Session::get_remote_listener_addrs(
    const std::string& host,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetRemoteListenerAddrs{
        host,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::PeerAddrs rsp = std::move(response.get<Response::PeerAddrs>());
    return rsp.value;
}

PublicRuntimeId Session::get_runtime_id(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetRuntimeId();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::PublicRuntimeId rsp = std::move(response.get<Response::PublicRuntimeId>());
    return rsp.value;
}

AccessMode Session::get_share_token_access_mode(
    const ShareToken& token,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetShareTokenAccessMode{
        token,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::AccessMode rsp = std::move(response.get<Response::AccessMode>());
    return rsp.value;
}

std::string Session::get_share_token_info_hash(
    const ShareToken& token,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetShareTokenInfoHash{
        token,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::String rsp = std::move(response.get<Response::String>());
    return rsp.value;
}

std::string Session::get_share_token_suggested_name(
    const ShareToken& token,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetShareTokenSuggestedName{
        token,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::String rsp = std::move(response.get<Response::String>());
    return rsp.value;
}

std::optional<StateMonitorNode> Session::get_state_monitor(
    const std::vector<MonitorId>& path,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetStateMonitor{
        path,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::StateMonitor>() == nullptr) return {};
    return std::move(response.get<Response::StateMonitor>()).value;
}

std::optional<std::string> Session::get_store_dir(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetStoreDir();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    if (response.get_if<Response::Path>() == nullptr) return {};
    return std::move(response.get<Response::Path>()).value;
}

std::vector<std::string> Session::get_user_provided_peers(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionGetUserProvidedPeers();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::PeerAddrs rsp = std::move(response.get<Response::PeerAddrs>());
    return rsp.value;
}

void Session::init_network(
    const NetworkDefaults& defaults,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionInitNetwork{
        defaults,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

bool Session::is_local_discovery_enabled(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionIsLocalDiscoveryEnabled();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Bool rsp = std::move(response.get<Response::Bool>());
    return rsp.value;
}

bool Session::is_pex_recv_enabled(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionIsPexRecvEnabled();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Bool rsp = std::move(response.get<Response::Bool>());
    return rsp.value;
}

bool Session::is_pex_send_enabled(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionIsPexSendEnabled();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Bool rsp = std::move(response.get<Response::Bool>());
    return rsp.value;
}

bool Session::is_port_forwarding_enabled(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionIsPortForwardingEnabled();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Bool rsp = std::move(response.get<Response::Bool>());
    return rsp.value;
}

std::map<std::string, Repository> Session::list_repositories(
    boost::asio::yield_context yield
) {
    auto request = Request::SessionListRepositories();
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Repositories rsp = std::move(response.get<Response::Repositories>());
    std::map<std::string, Repository> map;
    for (auto [k, v] : rsp.value) {
        map.emplace(std::make_pair(std::move(k), Repository(client, std::move(v))));
    }
    return map;
}

bool Session::mirror_exists(
    const ShareToken& token,
    const std::string& host,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionMirrorExists{
        token,
        host,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Bool rsp = std::move(response.get<Response::Bool>());
    return rsp.value;
}

Repository Session::open_repository(
    const std::string& path,
    boost::asio::yield_context yield,
    const std::optional<LocalSecret>& local_secret
) {
    auto request = Request::SessionOpenRepository{
        path,
        local_secret,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::Repository rsp = std::move(response.get<Response::Repository>());
    return Repository(client, std::move(rsp.value));
}

void Session::remove_user_provided_peers(
    const std::vector<std::string>& addrs,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionRemoveUserProvidedPeers{
        addrs,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Session::set_default_block_expiration(
    const std::optional<std::chrono::milliseconds>& value,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionSetDefaultBlockExpiration{
        value,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Session::set_default_quota(
    const std::optional<StorageSize>& value,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionSetDefaultQuota{
        value,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Session::set_default_repository_expiration(
    const std::optional<std::chrono::milliseconds>& value,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionSetDefaultRepositoryExpiration{
        value,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Session::set_local_discovery_enabled(
    bool enabled,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionSetLocalDiscoveryEnabled{
        enabled,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Session::set_mount_root(
    const std::optional<std::string>& path,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionSetMountRoot{
        path,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Session::set_pex_recv_enabled(
    bool enabled,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionSetPexRecvEnabled{
        enabled,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Session::set_pex_send_enabled(
    bool enabled,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionSetPexSendEnabled{
        enabled,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Session::set_port_forwarding_enabled(
    bool enabled,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionSetPortForwardingEnabled{
        enabled,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

void Session::set_store_dir(
    const std::string& path,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionSetStoreDir{
        path,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    response.get<Response::None>();
}

ShareToken Session::validate_share_token(
    const std::string& token,
    boost::asio::yield_context yield
) {
    auto request = Request::SessionValidateShareToken{
        token,
    };
    // TODO: This won't throw if yield has ec assigned
    auto response = client->invoke(request, yield);
    Response::ShareToken rsp = std::move(response.get<Response::ShareToken>());
    return rsp.value;
}

} // namespace ouisync
