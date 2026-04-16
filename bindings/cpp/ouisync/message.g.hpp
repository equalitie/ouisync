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
    struct Cancel {
        const ouisync::MessageId& id;
    };

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

    struct SessionPinDht {
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

    struct SessionUnpinDht {
    };

    struct SessionValidateShareToken {
        const std::string& token;
    };

    using Alternatives = std::variant<
        Cancel,
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
        SessionPinDht,
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
        SessionUnpinDht,
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

inline bool operator == (const Request::Cancel& lhs, const Request::Cancel& rhs) {
    return lhs.id == rhs.id;
}

inline bool operator != (const Request::Cancel& lhs, const Request::Cancel& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::FileClose& lhs, const Request::FileClose& rhs) {
    return lhs.file == rhs.file;
}

inline bool operator != (const Request::FileClose& lhs, const Request::FileClose& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::FileFlush& lhs, const Request::FileFlush& rhs) {
    return lhs.file == rhs.file;
}

inline bool operator != (const Request::FileFlush& lhs, const Request::FileFlush& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::FileGetLength& lhs, const Request::FileGetLength& rhs) {
    return lhs.file == rhs.file;
}

inline bool operator != (const Request::FileGetLength& lhs, const Request::FileGetLength& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::FileGetProgress& lhs, const Request::FileGetProgress& rhs) {
    return lhs.file == rhs.file;
}

inline bool operator != (const Request::FileGetProgress& lhs, const Request::FileGetProgress& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::FileRead& lhs, const Request::FileRead& rhs) {
    return lhs.file == rhs.file && lhs.offset == rhs.offset && lhs.size == rhs.size;
}

inline bool operator != (const Request::FileRead& lhs, const Request::FileRead& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::FileTruncate& lhs, const Request::FileTruncate& rhs) {
    return lhs.file == rhs.file && lhs.len == rhs.len;
}

inline bool operator != (const Request::FileTruncate& lhs, const Request::FileTruncate& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::FileWrite& lhs, const Request::FileWrite& rhs) {
    return lhs.file == rhs.file && lhs.offset == rhs.offset && lhs.data == rhs.data;
}

inline bool operator != (const Request::FileWrite& lhs, const Request::FileWrite& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::NetworkSocketClose& lhs, const Request::NetworkSocketClose& rhs) {
    return lhs.socket == rhs.socket;
}

inline bool operator != (const Request::NetworkSocketClose& lhs, const Request::NetworkSocketClose& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::NetworkSocketRecvFrom& lhs, const Request::NetworkSocketRecvFrom& rhs) {
    return lhs.socket == rhs.socket && lhs.len == rhs.len;
}

inline bool operator != (const Request::NetworkSocketRecvFrom& lhs, const Request::NetworkSocketRecvFrom& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::NetworkSocketSendTo& lhs, const Request::NetworkSocketSendTo& rhs) {
    return lhs.socket == rhs.socket && lhs.data == rhs.data && lhs.addr == rhs.addr;
}

inline bool operator != (const Request::NetworkSocketSendTo& lhs, const Request::NetworkSocketSendTo& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::NetworkStreamClose& lhs, const Request::NetworkStreamClose& rhs) {
    return lhs.stream == rhs.stream;
}

inline bool operator != (const Request::NetworkStreamClose& lhs, const Request::NetworkStreamClose& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::NetworkStreamReadExact& lhs, const Request::NetworkStreamReadExact& rhs) {
    return lhs.stream == rhs.stream && lhs.len == rhs.len;
}

inline bool operator != (const Request::NetworkStreamReadExact& lhs, const Request::NetworkStreamReadExact& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::NetworkStreamWriteAll& lhs, const Request::NetworkStreamWriteAll& rhs) {
    return lhs.stream == rhs.stream && lhs.buf == rhs.buf;
}

inline bool operator != (const Request::NetworkStreamWriteAll& lhs, const Request::NetworkStreamWriteAll& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryClose& lhs, const Request::RepositoryClose& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryClose& lhs, const Request::RepositoryClose& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryCreateDirectory& lhs, const Request::RepositoryCreateDirectory& rhs) {
    return lhs.repo == rhs.repo && lhs.path == rhs.path;
}

inline bool operator != (const Request::RepositoryCreateDirectory& lhs, const Request::RepositoryCreateDirectory& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryCreateFile& lhs, const Request::RepositoryCreateFile& rhs) {
    return lhs.repo == rhs.repo && lhs.path == rhs.path;
}

inline bool operator != (const Request::RepositoryCreateFile& lhs, const Request::RepositoryCreateFile& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryCreateMirror& lhs, const Request::RepositoryCreateMirror& rhs) {
    return lhs.repo == rhs.repo && lhs.host == rhs.host;
}

inline bool operator != (const Request::RepositoryCreateMirror& lhs, const Request::RepositoryCreateMirror& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryDelete& lhs, const Request::RepositoryDelete& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryDelete& lhs, const Request::RepositoryDelete& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryDeleteMirror& lhs, const Request::RepositoryDeleteMirror& rhs) {
    return lhs.repo == rhs.repo && lhs.host == rhs.host;
}

inline bool operator != (const Request::RepositoryDeleteMirror& lhs, const Request::RepositoryDeleteMirror& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryExport& lhs, const Request::RepositoryExport& rhs) {
    return lhs.repo == rhs.repo && lhs.output_path == rhs.output_path;
}

inline bool operator != (const Request::RepositoryExport& lhs, const Request::RepositoryExport& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryFileExists& lhs, const Request::RepositoryFileExists& rhs) {
    return lhs.repo == rhs.repo && lhs.path == rhs.path;
}

inline bool operator != (const Request::RepositoryFileExists& lhs, const Request::RepositoryFileExists& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryGetAccessMode& lhs, const Request::RepositoryGetAccessMode& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryGetAccessMode& lhs, const Request::RepositoryGetAccessMode& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryGetBlockExpiration& lhs, const Request::RepositoryGetBlockExpiration& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryGetBlockExpiration& lhs, const Request::RepositoryGetBlockExpiration& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryGetCredentials& lhs, const Request::RepositoryGetCredentials& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryGetCredentials& lhs, const Request::RepositoryGetCredentials& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryGetEntryType& lhs, const Request::RepositoryGetEntryType& rhs) {
    return lhs.repo == rhs.repo && lhs.path == rhs.path;
}

inline bool operator != (const Request::RepositoryGetEntryType& lhs, const Request::RepositoryGetEntryType& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryGetExpiration& lhs, const Request::RepositoryGetExpiration& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryGetExpiration& lhs, const Request::RepositoryGetExpiration& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryGetInfoHash& lhs, const Request::RepositoryGetInfoHash& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryGetInfoHash& lhs, const Request::RepositoryGetInfoHash& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryGetMetadata& lhs, const Request::RepositoryGetMetadata& rhs) {
    return lhs.repo == rhs.repo && lhs.key == rhs.key;
}

inline bool operator != (const Request::RepositoryGetMetadata& lhs, const Request::RepositoryGetMetadata& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryGetMountPoint& lhs, const Request::RepositoryGetMountPoint& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryGetMountPoint& lhs, const Request::RepositoryGetMountPoint& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryGetPath& lhs, const Request::RepositoryGetPath& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryGetPath& lhs, const Request::RepositoryGetPath& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryGetQuota& lhs, const Request::RepositoryGetQuota& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryGetQuota& lhs, const Request::RepositoryGetQuota& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryGetShortName& lhs, const Request::RepositoryGetShortName& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryGetShortName& lhs, const Request::RepositoryGetShortName& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryGetStats& lhs, const Request::RepositoryGetStats& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryGetStats& lhs, const Request::RepositoryGetStats& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryGetSyncProgress& lhs, const Request::RepositoryGetSyncProgress& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryGetSyncProgress& lhs, const Request::RepositoryGetSyncProgress& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryIsDhtEnabled& lhs, const Request::RepositoryIsDhtEnabled& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryIsDhtEnabled& lhs, const Request::RepositoryIsDhtEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryIsPexEnabled& lhs, const Request::RepositoryIsPexEnabled& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryIsPexEnabled& lhs, const Request::RepositoryIsPexEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryIsSyncEnabled& lhs, const Request::RepositoryIsSyncEnabled& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryIsSyncEnabled& lhs, const Request::RepositoryIsSyncEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryMirrorExists& lhs, const Request::RepositoryMirrorExists& rhs) {
    return lhs.repo == rhs.repo && lhs.host == rhs.host;
}

inline bool operator != (const Request::RepositoryMirrorExists& lhs, const Request::RepositoryMirrorExists& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryMount& lhs, const Request::RepositoryMount& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryMount& lhs, const Request::RepositoryMount& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryMove& lhs, const Request::RepositoryMove& rhs) {
    return lhs.repo == rhs.repo && lhs.dst == rhs.dst;
}

inline bool operator != (const Request::RepositoryMove& lhs, const Request::RepositoryMove& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryMoveEntry& lhs, const Request::RepositoryMoveEntry& rhs) {
    return lhs.repo == rhs.repo && lhs.src == rhs.src && lhs.dst == rhs.dst;
}

inline bool operator != (const Request::RepositoryMoveEntry& lhs, const Request::RepositoryMoveEntry& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryOpenFile& lhs, const Request::RepositoryOpenFile& rhs) {
    return lhs.repo == rhs.repo && lhs.path == rhs.path;
}

inline bool operator != (const Request::RepositoryOpenFile& lhs, const Request::RepositoryOpenFile& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryReadDirectory& lhs, const Request::RepositoryReadDirectory& rhs) {
    return lhs.repo == rhs.repo && lhs.path == rhs.path;
}

inline bool operator != (const Request::RepositoryReadDirectory& lhs, const Request::RepositoryReadDirectory& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryRemoveDirectory& lhs, const Request::RepositoryRemoveDirectory& rhs) {
    return lhs.repo == rhs.repo && lhs.path == rhs.path && lhs.recursive == rhs.recursive;
}

inline bool operator != (const Request::RepositoryRemoveDirectory& lhs, const Request::RepositoryRemoveDirectory& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryRemoveFile& lhs, const Request::RepositoryRemoveFile& rhs) {
    return lhs.repo == rhs.repo && lhs.path == rhs.path;
}

inline bool operator != (const Request::RepositoryRemoveFile& lhs, const Request::RepositoryRemoveFile& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryResetAccess& lhs, const Request::RepositoryResetAccess& rhs) {
    return lhs.repo == rhs.repo && lhs.token == rhs.token;
}

inline bool operator != (const Request::RepositoryResetAccess& lhs, const Request::RepositoryResetAccess& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositorySetAccess& lhs, const Request::RepositorySetAccess& rhs) {
    return lhs.repo == rhs.repo && lhs.read == rhs.read && lhs.write == rhs.write;
}

inline bool operator != (const Request::RepositorySetAccess& lhs, const Request::RepositorySetAccess& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositorySetAccessMode& lhs, const Request::RepositorySetAccessMode& rhs) {
    return lhs.repo == rhs.repo && lhs.access_mode == rhs.access_mode && lhs.local_secret == rhs.local_secret;
}

inline bool operator != (const Request::RepositorySetAccessMode& lhs, const Request::RepositorySetAccessMode& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositorySetBlockExpiration& lhs, const Request::RepositorySetBlockExpiration& rhs) {
    return lhs.repo == rhs.repo && lhs.value == rhs.value;
}

inline bool operator != (const Request::RepositorySetBlockExpiration& lhs, const Request::RepositorySetBlockExpiration& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositorySetCredentials& lhs, const Request::RepositorySetCredentials& rhs) {
    return lhs.repo == rhs.repo && lhs.credentials == rhs.credentials;
}

inline bool operator != (const Request::RepositorySetCredentials& lhs, const Request::RepositorySetCredentials& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositorySetDhtEnabled& lhs, const Request::RepositorySetDhtEnabled& rhs) {
    return lhs.repo == rhs.repo && lhs.enabled == rhs.enabled;
}

inline bool operator != (const Request::RepositorySetDhtEnabled& lhs, const Request::RepositorySetDhtEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositorySetExpiration& lhs, const Request::RepositorySetExpiration& rhs) {
    return lhs.repo == rhs.repo && lhs.value == rhs.value;
}

inline bool operator != (const Request::RepositorySetExpiration& lhs, const Request::RepositorySetExpiration& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositorySetMetadata& lhs, const Request::RepositorySetMetadata& rhs) {
    return lhs.repo == rhs.repo && lhs.edits == rhs.edits;
}

inline bool operator != (const Request::RepositorySetMetadata& lhs, const Request::RepositorySetMetadata& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositorySetPexEnabled& lhs, const Request::RepositorySetPexEnabled& rhs) {
    return lhs.repo == rhs.repo && lhs.enabled == rhs.enabled;
}

inline bool operator != (const Request::RepositorySetPexEnabled& lhs, const Request::RepositorySetPexEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositorySetQuota& lhs, const Request::RepositorySetQuota& rhs) {
    return lhs.repo == rhs.repo && lhs.value == rhs.value;
}

inline bool operator != (const Request::RepositorySetQuota& lhs, const Request::RepositorySetQuota& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositorySetSyncEnabled& lhs, const Request::RepositorySetSyncEnabled& rhs) {
    return lhs.repo == rhs.repo && lhs.enabled == rhs.enabled;
}

inline bool operator != (const Request::RepositorySetSyncEnabled& lhs, const Request::RepositorySetSyncEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryShare& lhs, const Request::RepositoryShare& rhs) {
    return lhs.repo == rhs.repo && lhs.access_mode == rhs.access_mode && lhs.local_secret == rhs.local_secret;
}

inline bool operator != (const Request::RepositoryShare& lhs, const Request::RepositoryShare& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositorySubscribe& lhs, const Request::RepositorySubscribe& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositorySubscribe& lhs, const Request::RepositorySubscribe& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::RepositoryUnmount& lhs, const Request::RepositoryUnmount& rhs) {
    return lhs.repo == rhs.repo;
}

inline bool operator != (const Request::RepositoryUnmount& lhs, const Request::RepositoryUnmount& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionAddUserProvidedPeers& lhs, const Request::SessionAddUserProvidedPeers& rhs) {
    return lhs.addrs == rhs.addrs;
}

inline bool operator != (const Request::SessionAddUserProvidedPeers& lhs, const Request::SessionAddUserProvidedPeers& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionBindMetrics& lhs, const Request::SessionBindMetrics& rhs) {
    return lhs.addr == rhs.addr;
}

inline bool operator != (const Request::SessionBindMetrics& lhs, const Request::SessionBindMetrics& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionBindNetwork& lhs, const Request::SessionBindNetwork& rhs) {
    return lhs.addrs == rhs.addrs;
}

inline bool operator != (const Request::SessionBindNetwork& lhs, const Request::SessionBindNetwork& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionBindRemoteControl& lhs, const Request::SessionBindRemoteControl& rhs) {
    return lhs.addr == rhs.addr;
}

inline bool operator != (const Request::SessionBindRemoteControl& lhs, const Request::SessionBindRemoteControl& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionCopy& lhs, const Request::SessionCopy& rhs) {
    return lhs.src_repo == rhs.src_repo && lhs.src_path == rhs.src_path && lhs.dst_repo == rhs.dst_repo && lhs.dst_path == rhs.dst_path;
}

inline bool operator != (const Request::SessionCopy& lhs, const Request::SessionCopy& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionCreateRepository& lhs, const Request::SessionCreateRepository& rhs) {
    return lhs.path == rhs.path && lhs.read_secret == rhs.read_secret && lhs.write_secret == rhs.write_secret && lhs.token == rhs.token && lhs.sync_enabled == rhs.sync_enabled && lhs.dht_enabled == rhs.dht_enabled && lhs.pex_enabled == rhs.pex_enabled;
}

inline bool operator != (const Request::SessionCreateRepository& lhs, const Request::SessionCreateRepository& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionDeleteRepositoryByName& lhs, const Request::SessionDeleteRepositoryByName& rhs) {
    return lhs.name == rhs.name;
}

inline bool operator != (const Request::SessionDeleteRepositoryByName& lhs, const Request::SessionDeleteRepositoryByName& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionDeriveSecretKey& lhs, const Request::SessionDeriveSecretKey& rhs) {
    return lhs.password == rhs.password && lhs.salt == rhs.salt;
}

inline bool operator != (const Request::SessionDeriveSecretKey& lhs, const Request::SessionDeriveSecretKey& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionDhtLookup& lhs, const Request::SessionDhtLookup& rhs) {
    return lhs.info_hash == rhs.info_hash && lhs.announce == rhs.announce;
}

inline bool operator != (const Request::SessionDhtLookup& lhs, const Request::SessionDhtLookup& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionFindRepository& lhs, const Request::SessionFindRepository& rhs) {
    return lhs.name == rhs.name;
}

inline bool operator != (const Request::SessionFindRepository& lhs, const Request::SessionFindRepository& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGeneratePasswordSalt& lhs, const Request::SessionGeneratePasswordSalt& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGeneratePasswordSalt& lhs, const Request::SessionGeneratePasswordSalt& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGenerateSecretKey& lhs, const Request::SessionGenerateSecretKey& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGenerateSecretKey& lhs, const Request::SessionGenerateSecretKey& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetCurrentProtocolVersion& lhs, const Request::SessionGetCurrentProtocolVersion& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetCurrentProtocolVersion& lhs, const Request::SessionGetCurrentProtocolVersion& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetDefaultBlockExpiration& lhs, const Request::SessionGetDefaultBlockExpiration& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetDefaultBlockExpiration& lhs, const Request::SessionGetDefaultBlockExpiration& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetDefaultQuota& lhs, const Request::SessionGetDefaultQuota& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetDefaultQuota& lhs, const Request::SessionGetDefaultQuota& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetDefaultRepositoryExpiration& lhs, const Request::SessionGetDefaultRepositoryExpiration& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetDefaultRepositoryExpiration& lhs, const Request::SessionGetDefaultRepositoryExpiration& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetDhtRouters& lhs, const Request::SessionGetDhtRouters& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetDhtRouters& lhs, const Request::SessionGetDhtRouters& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetExternalAddrV4& lhs, const Request::SessionGetExternalAddrV4& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetExternalAddrV4& lhs, const Request::SessionGetExternalAddrV4& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetExternalAddrV6& lhs, const Request::SessionGetExternalAddrV6& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetExternalAddrV6& lhs, const Request::SessionGetExternalAddrV6& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetHighestSeenProtocolVersion& lhs, const Request::SessionGetHighestSeenProtocolVersion& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetHighestSeenProtocolVersion& lhs, const Request::SessionGetHighestSeenProtocolVersion& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetLocalListenerAddrs& lhs, const Request::SessionGetLocalListenerAddrs& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetLocalListenerAddrs& lhs, const Request::SessionGetLocalListenerAddrs& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetMetricsListenerAddr& lhs, const Request::SessionGetMetricsListenerAddr& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetMetricsListenerAddr& lhs, const Request::SessionGetMetricsListenerAddr& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetMountRoot& lhs, const Request::SessionGetMountRoot& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetMountRoot& lhs, const Request::SessionGetMountRoot& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetNatBehavior& lhs, const Request::SessionGetNatBehavior& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetNatBehavior& lhs, const Request::SessionGetNatBehavior& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetNetworkStats& lhs, const Request::SessionGetNetworkStats& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetNetworkStats& lhs, const Request::SessionGetNetworkStats& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetPeers& lhs, const Request::SessionGetPeers& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetPeers& lhs, const Request::SessionGetPeers& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetRemoteControlListenerAddr& lhs, const Request::SessionGetRemoteControlListenerAddr& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetRemoteControlListenerAddr& lhs, const Request::SessionGetRemoteControlListenerAddr& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetRemoteListenerAddrs& lhs, const Request::SessionGetRemoteListenerAddrs& rhs) {
    return lhs.host == rhs.host;
}

inline bool operator != (const Request::SessionGetRemoteListenerAddrs& lhs, const Request::SessionGetRemoteListenerAddrs& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetRuntimeId& lhs, const Request::SessionGetRuntimeId& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetRuntimeId& lhs, const Request::SessionGetRuntimeId& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetShareTokenAccessMode& lhs, const Request::SessionGetShareTokenAccessMode& rhs) {
    return lhs.token == rhs.token;
}

inline bool operator != (const Request::SessionGetShareTokenAccessMode& lhs, const Request::SessionGetShareTokenAccessMode& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetShareTokenInfoHash& lhs, const Request::SessionGetShareTokenInfoHash& rhs) {
    return lhs.token == rhs.token;
}

inline bool operator != (const Request::SessionGetShareTokenInfoHash& lhs, const Request::SessionGetShareTokenInfoHash& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetShareTokenSuggestedName& lhs, const Request::SessionGetShareTokenSuggestedName& rhs) {
    return lhs.token == rhs.token;
}

inline bool operator != (const Request::SessionGetShareTokenSuggestedName& lhs, const Request::SessionGetShareTokenSuggestedName& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetStateMonitor& lhs, const Request::SessionGetStateMonitor& rhs) {
    return lhs.path == rhs.path;
}

inline bool operator != (const Request::SessionGetStateMonitor& lhs, const Request::SessionGetStateMonitor& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetStoreDirs& lhs, const Request::SessionGetStoreDirs& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetStoreDirs& lhs, const Request::SessionGetStoreDirs& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionGetUserProvidedPeers& lhs, const Request::SessionGetUserProvidedPeers& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionGetUserProvidedPeers& lhs, const Request::SessionGetUserProvidedPeers& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionInitNetwork& lhs, const Request::SessionInitNetwork& rhs) {
    return lhs.defaults == rhs.defaults;
}

inline bool operator != (const Request::SessionInitNetwork& lhs, const Request::SessionInitNetwork& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionInsertStoreDirs& lhs, const Request::SessionInsertStoreDirs& rhs) {
    return lhs.paths == rhs.paths;
}

inline bool operator != (const Request::SessionInsertStoreDirs& lhs, const Request::SessionInsertStoreDirs& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionIsLocalDhtEnabled& lhs, const Request::SessionIsLocalDhtEnabled& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionIsLocalDhtEnabled& lhs, const Request::SessionIsLocalDhtEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionIsLocalDiscoveryEnabled& lhs, const Request::SessionIsLocalDiscoveryEnabled& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionIsLocalDiscoveryEnabled& lhs, const Request::SessionIsLocalDiscoveryEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionIsPexRecvEnabled& lhs, const Request::SessionIsPexRecvEnabled& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionIsPexRecvEnabled& lhs, const Request::SessionIsPexRecvEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionIsPexSendEnabled& lhs, const Request::SessionIsPexSendEnabled& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionIsPexSendEnabled& lhs, const Request::SessionIsPexSendEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionIsPortForwardingEnabled& lhs, const Request::SessionIsPortForwardingEnabled& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionIsPortForwardingEnabled& lhs, const Request::SessionIsPortForwardingEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionListRepositories& lhs, const Request::SessionListRepositories& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionListRepositories& lhs, const Request::SessionListRepositories& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionMirrorExists& lhs, const Request::SessionMirrorExists& rhs) {
    return lhs.token == rhs.token && lhs.host == rhs.host;
}

inline bool operator != (const Request::SessionMirrorExists& lhs, const Request::SessionMirrorExists& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionOpenNetworkSocketV4& lhs, const Request::SessionOpenNetworkSocketV4& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionOpenNetworkSocketV4& lhs, const Request::SessionOpenNetworkSocketV4& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionOpenNetworkSocketV6& lhs, const Request::SessionOpenNetworkSocketV6& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionOpenNetworkSocketV6& lhs, const Request::SessionOpenNetworkSocketV6& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionOpenNetworkStream& lhs, const Request::SessionOpenNetworkStream& rhs) {
    return lhs.addr == rhs.addr && lhs.topic_id == rhs.topic_id;
}

inline bool operator != (const Request::SessionOpenNetworkStream& lhs, const Request::SessionOpenNetworkStream& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionOpenRepository& lhs, const Request::SessionOpenRepository& rhs) {
    return lhs.path == rhs.path && lhs.local_secret == rhs.local_secret;
}

inline bool operator != (const Request::SessionOpenRepository& lhs, const Request::SessionOpenRepository& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionPinDht& lhs, const Request::SessionPinDht& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionPinDht& lhs, const Request::SessionPinDht& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionRemoveStoreDirs& lhs, const Request::SessionRemoveStoreDirs& rhs) {
    return lhs.paths == rhs.paths;
}

inline bool operator != (const Request::SessionRemoveStoreDirs& lhs, const Request::SessionRemoveStoreDirs& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionRemoveUserProvidedPeers& lhs, const Request::SessionRemoveUserProvidedPeers& rhs) {
    return lhs.addrs == rhs.addrs;
}

inline bool operator != (const Request::SessionRemoveUserProvidedPeers& lhs, const Request::SessionRemoveUserProvidedPeers& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionSetDefaultBlockExpiration& lhs, const Request::SessionSetDefaultBlockExpiration& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Request::SessionSetDefaultBlockExpiration& lhs, const Request::SessionSetDefaultBlockExpiration& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionSetDefaultQuota& lhs, const Request::SessionSetDefaultQuota& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Request::SessionSetDefaultQuota& lhs, const Request::SessionSetDefaultQuota& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionSetDefaultRepositoryExpiration& lhs, const Request::SessionSetDefaultRepositoryExpiration& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Request::SessionSetDefaultRepositoryExpiration& lhs, const Request::SessionSetDefaultRepositoryExpiration& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionSetDhtRouters& lhs, const Request::SessionSetDhtRouters& rhs) {
    return lhs.routers == rhs.routers;
}

inline bool operator != (const Request::SessionSetDhtRouters& lhs, const Request::SessionSetDhtRouters& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionSetLocalDhtEnabled& lhs, const Request::SessionSetLocalDhtEnabled& rhs) {
    return lhs.enabled == rhs.enabled;
}

inline bool operator != (const Request::SessionSetLocalDhtEnabled& lhs, const Request::SessionSetLocalDhtEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionSetLocalDiscoveryEnabled& lhs, const Request::SessionSetLocalDiscoveryEnabled& rhs) {
    return lhs.enabled == rhs.enabled;
}

inline bool operator != (const Request::SessionSetLocalDiscoveryEnabled& lhs, const Request::SessionSetLocalDiscoveryEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionSetMountRoot& lhs, const Request::SessionSetMountRoot& rhs) {
    return lhs.path == rhs.path;
}

inline bool operator != (const Request::SessionSetMountRoot& lhs, const Request::SessionSetMountRoot& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionSetPexRecvEnabled& lhs, const Request::SessionSetPexRecvEnabled& rhs) {
    return lhs.enabled == rhs.enabled;
}

inline bool operator != (const Request::SessionSetPexRecvEnabled& lhs, const Request::SessionSetPexRecvEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionSetPexSendEnabled& lhs, const Request::SessionSetPexSendEnabled& rhs) {
    return lhs.enabled == rhs.enabled;
}

inline bool operator != (const Request::SessionSetPexSendEnabled& lhs, const Request::SessionSetPexSendEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionSetPortForwardingEnabled& lhs, const Request::SessionSetPortForwardingEnabled& rhs) {
    return lhs.enabled == rhs.enabled;
}

inline bool operator != (const Request::SessionSetPortForwardingEnabled& lhs, const Request::SessionSetPortForwardingEnabled& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionSetStoreDirs& lhs, const Request::SessionSetStoreDirs& rhs) {
    return lhs.paths == rhs.paths;
}

inline bool operator != (const Request::SessionSetStoreDirs& lhs, const Request::SessionSetStoreDirs& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionSubscribeToNetwork& lhs, const Request::SessionSubscribeToNetwork& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionSubscribeToNetwork& lhs, const Request::SessionSubscribeToNetwork& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionSubscribeToStateMonitor& lhs, const Request::SessionSubscribeToStateMonitor& rhs) {
    return lhs.path == rhs.path;
}

inline bool operator != (const Request::SessionSubscribeToStateMonitor& lhs, const Request::SessionSubscribeToStateMonitor& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionUnpinDht& lhs, const Request::SessionUnpinDht& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Request::SessionUnpinDht& lhs, const Request::SessionUnpinDht& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request::SessionValidateShareToken& lhs, const Request::SessionValidateShareToken& rhs) {
    return lhs.token == rhs.token;
}

inline bool operator != (const Request::SessionValidateShareToken& lhs, const Request::SessionValidateShareToken& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Request& lhs, const Request& rhs) {
    return lhs.value == rhs.value;
}

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

inline bool operator == (const Response::AccessMode& lhs, const Response::AccessMode& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::AccessMode& lhs, const Response::AccessMode& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::Bool& lhs, const Response::Bool& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::Bool& lhs, const Response::Bool& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::Bytes& lhs, const Response::Bytes& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::Bytes& lhs, const Response::Bytes& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::Datagram& lhs, const Response::Datagram& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::Datagram& lhs, const Response::Datagram& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::DirectoryEntries& lhs, const Response::DirectoryEntries& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::DirectoryEntries& lhs, const Response::DirectoryEntries& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::Duration& lhs, const Response::Duration& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::Duration& lhs, const Response::Duration& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::EntryType& lhs, const Response::EntryType& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::EntryType& lhs, const Response::EntryType& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::File& lhs, const Response::File& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::File& lhs, const Response::File& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::NatBehavior& lhs, const Response::NatBehavior& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::NatBehavior& lhs, const Response::NatBehavior& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::NetworkEvent& lhs, const Response::NetworkEvent& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::NetworkEvent& lhs, const Response::NetworkEvent& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::NetworkSocket& lhs, const Response::NetworkSocket& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::NetworkSocket& lhs, const Response::NetworkSocket& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::NetworkStream& lhs, const Response::NetworkStream& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::NetworkStream& lhs, const Response::NetworkStream& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::None& lhs, const Response::None& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Response::None& lhs, const Response::None& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::PasswordSalt& lhs, const Response::PasswordSalt& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::PasswordSalt& lhs, const Response::PasswordSalt& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::Path& lhs, const Response::Path& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::Path& lhs, const Response::Path& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::Paths& lhs, const Response::Paths& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::Paths& lhs, const Response::Paths& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::PeerAddr& lhs, const Response::PeerAddr& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::PeerAddr& lhs, const Response::PeerAddr& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::PeerAddrs& lhs, const Response::PeerAddrs& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::PeerAddrs& lhs, const Response::PeerAddrs& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::PeerInfos& lhs, const Response::PeerInfos& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::PeerInfos& lhs, const Response::PeerInfos& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::Progress& lhs, const Response::Progress& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::Progress& lhs, const Response::Progress& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::PublicRuntimeId& lhs, const Response::PublicRuntimeId& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::PublicRuntimeId& lhs, const Response::PublicRuntimeId& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::QuotaInfo& lhs, const Response::QuotaInfo& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::QuotaInfo& lhs, const Response::QuotaInfo& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::Repositories& lhs, const Response::Repositories& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::Repositories& lhs, const Response::Repositories& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::Repository& lhs, const Response::Repository& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::Repository& lhs, const Response::Repository& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::SecretKey& lhs, const Response::SecretKey& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::SecretKey& lhs, const Response::SecretKey& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::ShareToken& lhs, const Response::ShareToken& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::ShareToken& lhs, const Response::ShareToken& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::SocketAddr& lhs, const Response::SocketAddr& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::SocketAddr& lhs, const Response::SocketAddr& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::StateMonitor& lhs, const Response::StateMonitor& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::StateMonitor& lhs, const Response::StateMonitor& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::Stats& lhs, const Response::Stats& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::Stats& lhs, const Response::Stats& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::StorageSize& lhs, const Response::StorageSize& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::StorageSize& lhs, const Response::StorageSize& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::String& lhs, const Response::String& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::String& lhs, const Response::String& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::Strings& lhs, const Response::Strings& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::Strings& lhs, const Response::Strings& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::U16& lhs, const Response::U16& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::U16& lhs, const Response::U16& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::U64& lhs, const Response::U64& rhs) {
    return lhs.value == rhs.value;
}

inline bool operator != (const Response::U64& lhs, const Response::U64& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response::Unit& lhs, const Response::Unit& rhs) {
    (void) lhs; (void) rhs;
    return true;
}

inline bool operator != (const Response::Unit& lhs, const Response::Unit& rhs) {
    return !(lhs == rhs);
}

inline bool operator == (const Response& lhs, const Response& rhs) {
    return lhs.value == rhs.value;
}

} // namespace ouisync
