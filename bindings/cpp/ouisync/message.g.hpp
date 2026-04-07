// This file is auto generated. Do not edit.

#pragma once

#include <cstdint>
#include <optional>
#include <string>
#include <vector>
#include <ouisync/data.hpp>

namespace ouisync {
class Client;
class File;
class Repository;
class NetworkSocket;
class NetworkStream;
class Session;

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

    struct NetworkSocketClose {
        const ouisync::NetworkSocketHandle& socket;
    };

    struct NetworkSocketRecvFrom {
        const ouisync::NetworkSocketHandle& socket;
        uint64_t len;
    };

    struct NetworkSocketSendTo {
        const ouisync::NetworkSocketHandle& socket;
        const std::vector<uint8_t>& data;
        const std::string& addr;
    };

    struct NetworkStreamClose {
        const ouisync::NetworkStreamHandle& stream;
    };

    struct NetworkStreamReadExact {
        const ouisync::NetworkStreamHandle& stream;
        uint64_t len;
    };

    struct NetworkStreamWriteAll {
        const ouisync::NetworkStreamHandle& stream;
        const std::vector<uint8_t>& buf;
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
        const std::optional<std::string>& src_repo;
        const std::string& src_path;
        const std::optional<std::string>& dst_repo;
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

    struct SessionDhtLookup {
        const std::string& info_hash;
        bool announce;
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

    struct SessionGetDhtRouters {
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

    struct SessionIsLocalDhtEnabled {
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

    struct SessionOpenNetworkSocketV4 {
    };

    struct SessionOpenNetworkSocketV6 {
    };

    struct SessionOpenNetworkStream {
        const std::string& addr;
        const ouisync::TopicId& topic_id;
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

    struct SessionSetDhtRouters {
        const std::vector<std::string>& routers;
    };

    struct SessionSetLocalDhtEnabled {
        bool enabled;
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
        NetworkSocketClose,
        NetworkSocketRecvFrom,
        NetworkSocketSendTo,
        NetworkStreamClose,
        NetworkStreamReadExact,
        NetworkStreamWriteAll,
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
        SessionDhtLookup,
        SessionFindRepository,
        SessionGeneratePasswordSalt,
        SessionGenerateSecretKey,
        SessionGetCurrentProtocolVersion,
        SessionGetDefaultBlockExpiration,
        SessionGetDefaultQuota,
        SessionGetDefaultRepositoryExpiration,
        SessionGetDhtRouters,
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
        SessionIsLocalDhtEnabled,
        SessionIsLocalDiscoveryEnabled,
        SessionIsPexRecvEnabled,
        SessionIsPexSendEnabled,
        SessionIsPortForwardingEnabled,
        SessionListRepositories,
        SessionMirrorExists,
        SessionOpenNetworkSocketV4,
        SessionOpenNetworkSocketV6,
        SessionOpenNetworkStream,
        SessionOpenRepository,
        SessionRemoveStoreDirs,
        SessionRemoveUserProvidedPeers,
        SessionSetDefaultBlockExpiration,
        SessionSetDefaultQuota,
        SessionSetDefaultRepositoryExpiration,
        SessionSetDhtRouters,
        SessionSetLocalDhtEnabled,
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
    requires(!std::same_as<std::remove_cvref_t<T>, Request>)
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
        using type = ouisync::AccessMode;
        ouisync::AccessMode value;
    };

    struct Bool {
        using type = bool;
        bool value;
    };

    struct Bytes {
        using type = std::vector<uint8_t>;
        std::vector<uint8_t> value;
    };

    struct Datagram {
        using type = ouisync::Datagram;
        ouisync::Datagram value;
    };

    struct DirectoryEntries {
        using type = std::vector<ouisync::DirectoryEntry>;
        std::vector<ouisync::DirectoryEntry> value;
    };

    struct Duration {
        using type = std::chrono::milliseconds;
        std::chrono::milliseconds value;
    };

    struct EntryType {
        using type = ouisync::EntryType;
        ouisync::EntryType value;
    };

    struct File {
        using type = ouisync::File;
        ouisync::FileHandle value;
    };

    struct NatBehavior {
        using type = ouisync::NatBehavior;
        ouisync::NatBehavior value;
    };

    struct NetworkEvent {
        using type = ouisync::NetworkEvent;
        ouisync::NetworkEvent value;
    };

    struct NetworkSocket {
        using type = ouisync::NetworkSocket;
        ouisync::NetworkSocketHandle value;
    };

    struct NetworkStream {
        using type = ouisync::NetworkStream;
        ouisync::NetworkStreamHandle value;
    };

    struct None {
        using type = std::nullopt_t;
    };

    struct PasswordSalt {
        using type = ouisync::PasswordSalt;
        ouisync::PasswordSalt value;
    };

    struct Path {
        using type = std::string;
        std::string value;
    };

    struct Paths {
        using type = std::vector<std::string>;
        std::vector<std::string> value;
    };

    struct PeerAddr {
        using type = std::string;
        std::string value;
    };

    struct PeerAddrs {
        using type = std::vector<std::string>;
        std::vector<std::string> value;
    };

    struct PeerInfos {
        using type = std::vector<ouisync::PeerInfo>;
        std::vector<ouisync::PeerInfo> value;
    };

    struct Progress {
        using type = ouisync::Progress;
        ouisync::Progress value;
    };

    struct PublicRuntimeId {
        using type = ouisync::PublicRuntimeId;
        ouisync::PublicRuntimeId value;
    };

    struct QuotaInfo {
        using type = ouisync::QuotaInfo;
        ouisync::QuotaInfo value;
    };

    struct Repositories {
        using type = std::map<std::string, ouisync::Repository>;
        std::map<std::string, ouisync::RepositoryHandle> value;
    };

    struct Repository {
        using type = ouisync::Repository;
        ouisync::RepositoryHandle value;
    };

    struct SecretKey {
        using type = ouisync::SecretKey;
        ouisync::SecretKey value;
    };

    struct ShareToken {
        using type = ouisync::ShareToken;
        ouisync::ShareToken value;
    };

    struct SocketAddr {
        using type = std::string;
        std::string value;
    };

    struct StateMonitor {
        using type = ouisync::StateMonitorNode;
        ouisync::StateMonitorNode value;
    };

    struct Stats {
        using type = ouisync::Stats;
        ouisync::Stats value;
    };

    struct StorageSize {
        using type = ouisync::StorageSize;
        ouisync::StorageSize value;
    };

    struct String {
        using type = std::string;
        std::string value;
    };

    struct Strings {
        using type = std::vector<std::string>;
        std::vector<std::string> value;
    };

    struct U16 {
        using type = uint16_t;
        uint16_t value;
    };

    struct U64 {
        using type = uint64_t;
        uint64_t value;
    };

    struct Unit {
        using type = void;
    };

    using Alternatives = std::variant<
        AccessMode,
        Bool,
        Bytes,
        Datagram,
        DirectoryEntries,
        Duration,
        EntryType,
        File,
        NatBehavior,
        NetworkEvent,
        NetworkSocket,
        NetworkStream,
        None,
        PasswordSalt,
        Path,
        Paths,
        PeerAddr,
        PeerAddrs,
        PeerInfos,
        Progress,
        PublicRuntimeId,
        QuotaInfo,
        Repositories,
        Repository,
        SecretKey,
        ShareToken,
        SocketAddr,
        StateMonitor,
        Stats,
        StorageSize,
        String,
        Strings,
        U16,
        U64,
        Unit
    >;

    Alternatives value;

    Response() = default;
    Response(Response&&) = default;
    Response& operator=(Response&&) = default;
    Response(Response const&) = default;

    template<class T>
    requires(!std::same_as<std::remove_cvref_t<T>, Response>)
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
