// This file is auto generated. Do not edit.

#pragma once

#include <string_view>

namespace ouisync {

struct Request {
    struct FileClose {
        const ouisync::FileHandle& file;
    };

    struct FileFlush {
        const ouisync::FileHandle& file;
    };

    struct FileGetLength {
        const ouisync::FileHandle& file;
    };

    struct FileGetProgress {
        const ouisync::FileHandle& file;
    };

    struct FileRead {
        const ouisync::FileHandle& file;
        uint64_t offset;
        uint64_t size;
    };

    struct FileTruncate {
        const ouisync::FileHandle& file;
        uint64_t len;
    };

    struct FileWrite {
        const ouisync::FileHandle& file;
        uint64_t offset;
        const std::vector<uint8_t>& data;
    };

    struct RepositoryClose {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryCreateDirectory {
        const ouisync::RepositoryHandle& repo;
        const std::string& path;
    };

    struct RepositoryCreateFile {
        const ouisync::RepositoryHandle& repo;
        const std::string& path;
    };

    struct RepositoryCreateMirror {
        const ouisync::RepositoryHandle& repo;
        const std::string& host;
    };

    struct RepositoryDelete {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryDeleteMirror {
        const ouisync::RepositoryHandle& repo;
        const std::string& host;
    };

    struct RepositoryExport {
        const ouisync::RepositoryHandle& repo;
        const std::string& output_path;
    };

    struct RepositoryFileExists {
        const ouisync::RepositoryHandle& repo;
        const std::string& path;
    };

    struct RepositoryGetAccessMode {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryGetBlockExpiration {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryGetCredentials {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryGetEntryType {
        const ouisync::RepositoryHandle& repo;
        const std::string& path;
    };

    struct RepositoryGetExpiration {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryGetInfoHash {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryGetMetadata {
        const ouisync::RepositoryHandle& repo;
        const std::string& key;
    };

    struct RepositoryGetMountPoint {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryGetPath {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryGetQuota {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryGetShortName {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryGetStats {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryGetSyncProgress {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryIsDhtEnabled {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryIsPexEnabled {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryIsSyncEnabled {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryMirrorExists {
        const ouisync::RepositoryHandle& repo;
        const std::string& host;
    };

    struct RepositoryMount {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryMove {
        const ouisync::RepositoryHandle& repo;
        const std::string& dst;
    };

    struct RepositoryMoveEntry {
        const ouisync::RepositoryHandle& repo;
        const std::string& src;
        const std::string& dst;
    };

    struct RepositoryOpenFile {
        const ouisync::RepositoryHandle& repo;
        const std::string& path;
    };

    struct RepositoryReadDirectory {
        const ouisync::RepositoryHandle& repo;
        const std::string& path;
    };

    struct RepositoryRemoveDirectory {
        const ouisync::RepositoryHandle& repo;
        const std::string& path;
        bool recursive;
    };

    struct RepositoryRemoveFile {
        const ouisync::RepositoryHandle& repo;
        const std::string& path;
    };

    struct RepositoryResetAccess {
        const ouisync::RepositoryHandle& repo;
        const ouisync::ShareToken& token;
    };

    struct RepositorySetAccess {
        const ouisync::RepositoryHandle& repo;
        const std::optional<ouisync::AccessChange>& read;
        const std::optional<ouisync::AccessChange>& write;
    };

    struct RepositorySetAccessMode {
        const ouisync::RepositoryHandle& repo;
        const ouisync::AccessMode& access_mode;
        const std::optional<ouisync::LocalSecret>& local_secret;
    };

    struct RepositorySetBlockExpiration {
        const ouisync::RepositoryHandle& repo;
        const std::optional<std::chrono::milliseconds>& value;
    };

    struct RepositorySetCredentials {
        const ouisync::RepositoryHandle& repo;
        const std::vector<uint8_t>& credentials;
    };

    struct RepositorySetDhtEnabled {
        const ouisync::RepositoryHandle& repo;
        bool enabled;
    };

    struct RepositorySetExpiration {
        const ouisync::RepositoryHandle& repo;
        const std::optional<std::chrono::milliseconds>& value;
    };

    struct RepositorySetMetadata {
        const ouisync::RepositoryHandle& repo;
        const std::vector<ouisync::MetadataEdit>& edits;
    };

    struct RepositorySetPexEnabled {
        const ouisync::RepositoryHandle& repo;
        bool enabled;
    };

    struct RepositorySetQuota {
        const ouisync::RepositoryHandle& repo;
        const std::optional<ouisync::StorageSize>& value;
    };

    struct RepositorySetSyncEnabled {
        const ouisync::RepositoryHandle& repo;
        bool enabled;
    };

    struct RepositoryShare {
        const ouisync::RepositoryHandle& repo;
        const ouisync::AccessMode& access_mode;
        const std::optional<ouisync::LocalSecret>& local_secret;
    };

    struct RepositorySubscribe {
        const ouisync::RepositoryHandle& repo;
    };

    struct RepositoryUnmount {
        const ouisync::RepositoryHandle& repo;
    };

    struct SessionAddUserProvidedPeers {
        const std::vector<std::string>& addrs;
    };

    struct SessionBindMetrics {
        const std::optional<std::string>& addr;
    };

    struct SessionBindNetwork {
        const std::vector<std::string>& addrs;
    };

    struct SessionBindRemoteControl {
        const std::optional<std::string>& addr;
    };

    struct SessionCopy {
        const std::string& src_path;
        const std::string& dst_repo_name;
        const std::string& dst_path;
    };

    struct SessionCreateRepository {
        const std::string& path;
        const std::optional<ouisync::SetLocalSecret>& read_secret;
        const std::optional<ouisync::SetLocalSecret>& write_secret;
        const std::optional<ouisync::ShareToken>& token;
        bool sync_enabled;
        bool dht_enabled;
        bool pex_enabled;
    };

    struct SessionDeleteRepositoryByName {
        const std::string& name;
    };

    struct SessionDeriveSecretKey {
        const ouisync::Password& password;
        const ouisync::PasswordSalt& salt;
    };

    struct SessionFindRepository {
        const std::string& name;
    };

    struct SessionGeneratePasswordSalt {
    };

    struct SessionGenerateSecretKey {
    };

    struct SessionGetCurrentProtocolVersion {
    };

    struct SessionGetDefaultBlockExpiration {
    };

    struct SessionGetDefaultQuota {
    };

    struct SessionGetDefaultRepositoryExpiration {
    };

    struct SessionGetExternalAddrV4 {
    };

    struct SessionGetExternalAddrV6 {
    };

    struct SessionGetHighestSeenProtocolVersion {
    };

    struct SessionGetLocalListenerAddrs {
    };

    struct SessionGetMetricsListenerAddr {
    };

    struct SessionGetMountRoot {
    };

    struct SessionGetNatBehavior {
    };

    struct SessionGetNetworkStats {
    };

    struct SessionGetPeers {
    };

    struct SessionGetRemoteControlListenerAddr {
    };

    struct SessionGetRemoteListenerAddrs {
        const std::string& host;
    };

    struct SessionGetRuntimeId {
    };

    struct SessionGetShareTokenAccessMode {
        const ouisync::ShareToken& token;
    };

    struct SessionGetShareTokenInfoHash {
        const ouisync::ShareToken& token;
    };

    struct SessionGetShareTokenSuggestedName {
        const ouisync::ShareToken& token;
    };

    struct SessionGetStateMonitor {
        const std::vector<ouisync::MonitorId>& path;
    };

    struct SessionGetStoreDirs {
    };

    struct SessionGetUserProvidedPeers {
    };

    struct SessionInitNetwork {
        const ouisync::NetworkDefaults& defaults;
    };

    struct SessionInsertStoreDirs {
        const std::vector<std::string>& paths;
    };

    struct SessionIsLocalDiscoveryEnabled {
    };

    struct SessionIsPexRecvEnabled {
    };

    struct SessionIsPexSendEnabled {
    };

    struct SessionIsPortForwardingEnabled {
    };

    struct SessionListRepositories {
    };

    struct SessionMirrorExists {
        const ouisync::ShareToken& token;
        const std::string& host;
    };

    struct SessionOpenRepository {
        const std::string& path;
        const std::optional<ouisync::LocalSecret>& local_secret;
    };

    struct SessionRemoveStoreDirs {
        const std::vector<std::string>& paths;
    };

    struct SessionRemoveUserProvidedPeers {
        const std::vector<std::string>& addrs;
    };

    struct SessionSetDefaultBlockExpiration {
        const std::optional<std::chrono::milliseconds>& value;
    };

    struct SessionSetDefaultQuota {
        const std::optional<ouisync::StorageSize>& value;
    };

    struct SessionSetDefaultRepositoryExpiration {
        const std::optional<std::chrono::milliseconds>& value;
    };

    struct SessionSetLocalDiscoveryEnabled {
        bool enabled;
    };

    struct SessionSetMountRoot {
        const std::optional<std::string>& path;
    };

    struct SessionSetPexRecvEnabled {
        bool enabled;
    };

    struct SessionSetPexSendEnabled {
        bool enabled;
    };

    struct SessionSetPortForwardingEnabled {
        bool enabled;
    };

    struct SessionSetStoreDirs {
        const std::vector<std::string>& paths;
    };

    struct SessionSubscribeToNetwork {
    };

    struct SessionSubscribeToStateMonitor {
        const std::vector<ouisync::MonitorId>& path;
    };

    struct SessionUnsubscribe {
        const ouisync::MessageId& id;
    };

    struct SessionValidateShareToken {
        const std::string& token;
    };

    using Alternatives = std::variant<
        FileClose,
        FileFlush,
        FileGetLength,
        FileGetProgress,
        FileRead,
        FileTruncate,
        FileWrite,
        RepositoryClose,
        RepositoryCreateDirectory,
        RepositoryCreateFile,
        RepositoryCreateMirror,
        RepositoryDelete,
        RepositoryDeleteMirror,
        RepositoryExport,
        RepositoryFileExists,
        RepositoryGetAccessMode,
        RepositoryGetBlockExpiration,
        RepositoryGetCredentials,
        RepositoryGetEntryType,
        RepositoryGetExpiration,
        RepositoryGetInfoHash,
        RepositoryGetMetadata,
        RepositoryGetMountPoint,
        RepositoryGetPath,
        RepositoryGetQuota,
        RepositoryGetShortName,
        RepositoryGetStats,
        RepositoryGetSyncProgress,
        RepositoryIsDhtEnabled,
        RepositoryIsPexEnabled,
        RepositoryIsSyncEnabled,
        RepositoryMirrorExists,
        RepositoryMount,
        RepositoryMove,
        RepositoryMoveEntry,
        RepositoryOpenFile,
        RepositoryReadDirectory,
        RepositoryRemoveDirectory,
        RepositoryRemoveFile,
        RepositoryResetAccess,
        RepositorySetAccess,
        RepositorySetAccessMode,
        RepositorySetBlockExpiration,
        RepositorySetCredentials,
        RepositorySetDhtEnabled,
        RepositorySetExpiration,
        RepositorySetMetadata,
        RepositorySetPexEnabled,
        RepositorySetQuota,
        RepositorySetSyncEnabled,
        RepositoryShare,
        RepositorySubscribe,
        RepositoryUnmount,
        SessionAddUserProvidedPeers,
        SessionBindMetrics,
        SessionBindNetwork,
        SessionBindRemoteControl,
        SessionCopy,
        SessionCreateRepository,
        SessionDeleteRepositoryByName,
        SessionDeriveSecretKey,
        SessionFindRepository,
        SessionGeneratePasswordSalt,
        SessionGenerateSecretKey,
        SessionGetCurrentProtocolVersion,
        SessionGetDefaultBlockExpiration,
        SessionGetDefaultQuota,
        SessionGetDefaultRepositoryExpiration,
        SessionGetExternalAddrV4,
        SessionGetExternalAddrV6,
        SessionGetHighestSeenProtocolVersion,
        SessionGetLocalListenerAddrs,
        SessionGetMetricsListenerAddr,
        SessionGetMountRoot,
        SessionGetNatBehavior,
        SessionGetNetworkStats,
        SessionGetPeers,
        SessionGetRemoteControlListenerAddr,
        SessionGetRemoteListenerAddrs,
        SessionGetRuntimeId,
        SessionGetShareTokenAccessMode,
        SessionGetShareTokenInfoHash,
        SessionGetShareTokenSuggestedName,
        SessionGetStateMonitor,
        SessionGetStoreDirs,
        SessionGetUserProvidedPeers,
        SessionInitNetwork,
        SessionInsertStoreDirs,
        SessionIsLocalDiscoveryEnabled,
        SessionIsPexRecvEnabled,
        SessionIsPexSendEnabled,
        SessionIsPortForwardingEnabled,
        SessionListRepositories,
        SessionMirrorExists,
        SessionOpenRepository,
        SessionRemoveStoreDirs,
        SessionRemoveUserProvidedPeers,
        SessionSetDefaultBlockExpiration,
        SessionSetDefaultQuota,
        SessionSetDefaultRepositoryExpiration,
        SessionSetLocalDiscoveryEnabled,
        SessionSetMountRoot,
        SessionSetPexRecvEnabled,
        SessionSetPexSendEnabled,
        SessionSetPortForwardingEnabled,
        SessionSetStoreDirs,
        SessionSubscribeToNetwork,
        SessionSubscribeToStateMonitor,
        SessionUnsubscribe,
        SessionValidateShareToken
    >;

    Alternatives value;

    Request() = default;
    Request(Request&&) = default;
    Request& operator=(Request&&) = default;
    Request(Request const&) = default;

    template<class T>
    Request(T&& v)
        : value(std::forward<T>(v)) {}

    template<class T>
    T& get() {
        return std::get<T>(value);
    }

    template<class T>
    T* get_if() {
        return std::get_if<T>(&value);
    }

};
struct Response {
    struct AccessMode {
        ouisync::AccessMode value;
    };

    struct Bool {
        bool value;
    };

    struct Bytes {
        std::vector<uint8_t> value;
    };

    struct DirectoryEntries {
        std::vector<ouisync::DirectoryEntry> value;
    };

    struct Duration {
        std::chrono::milliseconds value;
    };

    struct EntryType {
        ouisync::EntryType value;
    };

    struct File {
        ouisync::FileHandle value;
    };

    struct NatBehavior {
        ouisync::NatBehavior value;
    };

    struct NetworkEvent {
        ouisync::NetworkEvent value;
    };

    struct None {
    };

    struct PasswordSalt {
        ouisync::PasswordSalt value;
    };

    struct Path {
        std::string value;
    };

    struct Paths {
        std::vector<std::string> value;
    };

    struct PeerAddrs {
        std::vector<std::string> value;
    };

    struct PeerInfos {
        std::vector<ouisync::PeerInfo> value;
    };

    struct Progress {
        ouisync::Progress value;
    };

    struct PublicRuntimeId {
        ouisync::PublicRuntimeId value;
    };

    struct QuotaInfo {
        ouisync::QuotaInfo value;
    };

    struct Repositories {
        std::map<std::string, ouisync::RepositoryHandle> value;
    };

    struct Repository {
        ouisync::RepositoryHandle value;
    };

    struct RepositoryEvent {
    };

    struct SecretKey {
        ouisync::SecretKey value;
    };

    struct ShareToken {
        ouisync::ShareToken value;
    };

    struct SocketAddr {
        std::string value;
    };

    struct StateMonitor {
        ouisync::StateMonitorNode value;
    };

    struct StateMonitorEvent {
    };

    struct Stats {
        ouisync::Stats value;
    };

    struct StorageSize {
        ouisync::StorageSize value;
    };

    struct String {
        std::string value;
    };

    struct U16 {
        uint16_t value;
    };

    struct U64 {
        uint64_t value;
    };

    using Alternatives = std::variant<
        AccessMode,
        Bool,
        Bytes,
        DirectoryEntries,
        Duration,
        EntryType,
        File,
        NatBehavior,
        NetworkEvent,
        None,
        PasswordSalt,
        Path,
        Paths,
        PeerAddrs,
        PeerInfos,
        Progress,
        PublicRuntimeId,
        QuotaInfo,
        Repositories,
        Repository,
        RepositoryEvent,
        SecretKey,
        ShareToken,
        SocketAddr,
        StateMonitor,
        StateMonitorEvent,
        Stats,
        StorageSize,
        String,
        U16,
        U64
    >;

    Alternatives value;

    Response() = default;
    Response(Response&&) = default;
    Response& operator=(Response&&) = default;
    Response(Response const&) = default;

    template<class T>
    Response(T&& v)
        : value(std::forward<T>(v)) {}

    template<class T>
    T& get() {
        return std::get<T>(value);
    }

    template<class T>
    T* get_if() {
        return std::get_if<T>(&value);
    }

};
} // namespace ouisync
