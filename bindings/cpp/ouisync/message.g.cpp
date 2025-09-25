// This file is auto generated. Do not edit.

#include <ouisync/data.hpp>
#include <ouisync/message.g.hpp>

namespace ouisync {

std::string_view variant_name(const Request::Alternatives& variant) {
    if (std::get_if<Request::FileClose>(&variant) != nullptr) {
        return std::string_view("FileClose");
    }
    if (std::get_if<Request::FileFlush>(&variant) != nullptr) {
        return std::string_view("FileFlush");
    }
    if (std::get_if<Request::FileGetLength>(&variant) != nullptr) {
        return std::string_view("FileGetLength");
    }
    if (std::get_if<Request::FileGetProgress>(&variant) != nullptr) {
        return std::string_view("FileGetProgress");
    }
    if (std::get_if<Request::FileRead>(&variant) != nullptr) {
        return std::string_view("FileRead");
    }
    if (std::get_if<Request::FileTruncate>(&variant) != nullptr) {
        return std::string_view("FileTruncate");
    }
    if (std::get_if<Request::FileWrite>(&variant) != nullptr) {
        return std::string_view("FileWrite");
    }
    if (std::get_if<Request::RepositoryClose>(&variant) != nullptr) {
        return std::string_view("RepositoryClose");
    }
    if (std::get_if<Request::RepositoryCreateDirectory>(&variant) != nullptr) {
        return std::string_view("RepositoryCreateDirectory");
    }
    if (std::get_if<Request::RepositoryCreateFile>(&variant) != nullptr) {
        return std::string_view("RepositoryCreateFile");
    }
    if (std::get_if<Request::RepositoryCreateMirror>(&variant) != nullptr) {
        return std::string_view("RepositoryCreateMirror");
    }
    if (std::get_if<Request::RepositoryDelete>(&variant) != nullptr) {
        return std::string_view("RepositoryDelete");
    }
    if (std::get_if<Request::RepositoryDeleteMirror>(&variant) != nullptr) {
        return std::string_view("RepositoryDeleteMirror");
    }
    if (std::get_if<Request::RepositoryExport>(&variant) != nullptr) {
        return std::string_view("RepositoryExport");
    }
    if (std::get_if<Request::RepositoryFileExists>(&variant) != nullptr) {
        return std::string_view("RepositoryFileExists");
    }
    if (std::get_if<Request::RepositoryGetAccessMode>(&variant) != nullptr) {
        return std::string_view("RepositoryGetAccessMode");
    }
    if (std::get_if<Request::RepositoryGetBlockExpiration>(&variant) != nullptr) {
        return std::string_view("RepositoryGetBlockExpiration");
    }
    if (std::get_if<Request::RepositoryGetCredentials>(&variant) != nullptr) {
        return std::string_view("RepositoryGetCredentials");
    }
    if (std::get_if<Request::RepositoryGetEntryType>(&variant) != nullptr) {
        return std::string_view("RepositoryGetEntryType");
    }
    if (std::get_if<Request::RepositoryGetExpiration>(&variant) != nullptr) {
        return std::string_view("RepositoryGetExpiration");
    }
    if (std::get_if<Request::RepositoryGetInfoHash>(&variant) != nullptr) {
        return std::string_view("RepositoryGetInfoHash");
    }
    if (std::get_if<Request::RepositoryGetMetadata>(&variant) != nullptr) {
        return std::string_view("RepositoryGetMetadata");
    }
    if (std::get_if<Request::RepositoryGetMountPoint>(&variant) != nullptr) {
        return std::string_view("RepositoryGetMountPoint");
    }
    if (std::get_if<Request::RepositoryGetPath>(&variant) != nullptr) {
        return std::string_view("RepositoryGetPath");
    }
    if (std::get_if<Request::RepositoryGetQuota>(&variant) != nullptr) {
        return std::string_view("RepositoryGetQuota");
    }
    if (std::get_if<Request::RepositoryGetShortName>(&variant) != nullptr) {
        return std::string_view("RepositoryGetShortName");
    }
    if (std::get_if<Request::RepositoryGetStats>(&variant) != nullptr) {
        return std::string_view("RepositoryGetStats");
    }
    if (std::get_if<Request::RepositoryGetSyncProgress>(&variant) != nullptr) {
        return std::string_view("RepositoryGetSyncProgress");
    }
    if (std::get_if<Request::RepositoryIsDhtEnabled>(&variant) != nullptr) {
        return std::string_view("RepositoryIsDhtEnabled");
    }
    if (std::get_if<Request::RepositoryIsPexEnabled>(&variant) != nullptr) {
        return std::string_view("RepositoryIsPexEnabled");
    }
    if (std::get_if<Request::RepositoryIsSyncEnabled>(&variant) != nullptr) {
        return std::string_view("RepositoryIsSyncEnabled");
    }
    if (std::get_if<Request::RepositoryMirrorExists>(&variant) != nullptr) {
        return std::string_view("RepositoryMirrorExists");
    }
    if (std::get_if<Request::RepositoryMount>(&variant) != nullptr) {
        return std::string_view("RepositoryMount");
    }
    if (std::get_if<Request::RepositoryMove>(&variant) != nullptr) {
        return std::string_view("RepositoryMove");
    }
    if (std::get_if<Request::RepositoryMoveEntry>(&variant) != nullptr) {
        return std::string_view("RepositoryMoveEntry");
    }
    if (std::get_if<Request::RepositoryOpenFile>(&variant) != nullptr) {
        return std::string_view("RepositoryOpenFile");
    }
    if (std::get_if<Request::RepositoryReadDirectory>(&variant) != nullptr) {
        return std::string_view("RepositoryReadDirectory");
    }
    if (std::get_if<Request::RepositoryRemoveDirectory>(&variant) != nullptr) {
        return std::string_view("RepositoryRemoveDirectory");
    }
    if (std::get_if<Request::RepositoryRemoveFile>(&variant) != nullptr) {
        return std::string_view("RepositoryRemoveFile");
    }
    if (std::get_if<Request::RepositoryResetAccess>(&variant) != nullptr) {
        return std::string_view("RepositoryResetAccess");
    }
    if (std::get_if<Request::RepositorySetAccess>(&variant) != nullptr) {
        return std::string_view("RepositorySetAccess");
    }
    if (std::get_if<Request::RepositorySetAccessMode>(&variant) != nullptr) {
        return std::string_view("RepositorySetAccessMode");
    }
    if (std::get_if<Request::RepositorySetBlockExpiration>(&variant) != nullptr) {
        return std::string_view("RepositorySetBlockExpiration");
    }
    if (std::get_if<Request::RepositorySetCredentials>(&variant) != nullptr) {
        return std::string_view("RepositorySetCredentials");
    }
    if (std::get_if<Request::RepositorySetDhtEnabled>(&variant) != nullptr) {
        return std::string_view("RepositorySetDhtEnabled");
    }
    if (std::get_if<Request::RepositorySetExpiration>(&variant) != nullptr) {
        return std::string_view("RepositorySetExpiration");
    }
    if (std::get_if<Request::RepositorySetMetadata>(&variant) != nullptr) {
        return std::string_view("RepositorySetMetadata");
    }
    if (std::get_if<Request::RepositorySetPexEnabled>(&variant) != nullptr) {
        return std::string_view("RepositorySetPexEnabled");
    }
    if (std::get_if<Request::RepositorySetQuota>(&variant) != nullptr) {
        return std::string_view("RepositorySetQuota");
    }
    if (std::get_if<Request::RepositorySetSyncEnabled>(&variant) != nullptr) {
        return std::string_view("RepositorySetSyncEnabled");
    }
    if (std::get_if<Request::RepositoryShare>(&variant) != nullptr) {
        return std::string_view("RepositoryShare");
    }
    if (std::get_if<Request::RepositorySubscribe>(&variant) != nullptr) {
        return std::string_view("RepositorySubscribe");
    }
    if (std::get_if<Request::RepositoryUnmount>(&variant) != nullptr) {
        return std::string_view("RepositoryUnmount");
    }
    if (std::get_if<Request::SessionAddUserProvidedPeers>(&variant) != nullptr) {
        return std::string_view("SessionAddUserProvidedPeers");
    }
    if (std::get_if<Request::SessionBindMetrics>(&variant) != nullptr) {
        return std::string_view("SessionBindMetrics");
    }
    if (std::get_if<Request::SessionBindNetwork>(&variant) != nullptr) {
        return std::string_view("SessionBindNetwork");
    }
    if (std::get_if<Request::SessionBindRemoteControl>(&variant) != nullptr) {
        return std::string_view("SessionBindRemoteControl");
    }
    if (std::get_if<Request::SessionCreateRepository>(&variant) != nullptr) {
        return std::string_view("SessionCreateRepository");
    }
    if (std::get_if<Request::SessionDeleteRepositoryByName>(&variant) != nullptr) {
        return std::string_view("SessionDeleteRepositoryByName");
    }
    if (std::get_if<Request::SessionDeriveSecretKey>(&variant) != nullptr) {
        return std::string_view("SessionDeriveSecretKey");
    }
    if (std::get_if<Request::SessionFindRepository>(&variant) != nullptr) {
        return std::string_view("SessionFindRepository");
    }
    if (std::get_if<Request::SessionGeneratePasswordSalt>(&variant) != nullptr) {
        return std::string_view("SessionGeneratePasswordSalt");
    }
    if (std::get_if<Request::SessionGenerateSecretKey>(&variant) != nullptr) {
        return std::string_view("SessionGenerateSecretKey");
    }
    if (std::get_if<Request::SessionGetCurrentProtocolVersion>(&variant) != nullptr) {
        return std::string_view("SessionGetCurrentProtocolVersion");
    }
    if (std::get_if<Request::SessionGetDefaultBlockExpiration>(&variant) != nullptr) {
        return std::string_view("SessionGetDefaultBlockExpiration");
    }
    if (std::get_if<Request::SessionGetDefaultQuota>(&variant) != nullptr) {
        return std::string_view("SessionGetDefaultQuota");
    }
    if (std::get_if<Request::SessionGetDefaultRepositoryExpiration>(&variant) != nullptr) {
        return std::string_view("SessionGetDefaultRepositoryExpiration");
    }
    if (std::get_if<Request::SessionGetExternalAddrV4>(&variant) != nullptr) {
        return std::string_view("SessionGetExternalAddrV4");
    }
    if (std::get_if<Request::SessionGetExternalAddrV6>(&variant) != nullptr) {
        return std::string_view("SessionGetExternalAddrV6");
    }
    if (std::get_if<Request::SessionGetHighestSeenProtocolVersion>(&variant) != nullptr) {
        return std::string_view("SessionGetHighestSeenProtocolVersion");
    }
    if (std::get_if<Request::SessionGetLocalListenerAddrs>(&variant) != nullptr) {
        return std::string_view("SessionGetLocalListenerAddrs");
    }
    if (std::get_if<Request::SessionGetMetricsListenerAddr>(&variant) != nullptr) {
        return std::string_view("SessionGetMetricsListenerAddr");
    }
    if (std::get_if<Request::SessionGetMountRoot>(&variant) != nullptr) {
        return std::string_view("SessionGetMountRoot");
    }
    if (std::get_if<Request::SessionGetNatBehavior>(&variant) != nullptr) {
        return std::string_view("SessionGetNatBehavior");
    }
    if (std::get_if<Request::SessionGetNetworkStats>(&variant) != nullptr) {
        return std::string_view("SessionGetNetworkStats");
    }
    if (std::get_if<Request::SessionGetPeers>(&variant) != nullptr) {
        return std::string_view("SessionGetPeers");
    }
    if (std::get_if<Request::SessionGetRemoteControlListenerAddr>(&variant) != nullptr) {
        return std::string_view("SessionGetRemoteControlListenerAddr");
    }
    if (std::get_if<Request::SessionGetRemoteListenerAddrs>(&variant) != nullptr) {
        return std::string_view("SessionGetRemoteListenerAddrs");
    }
    if (std::get_if<Request::SessionGetRuntimeId>(&variant) != nullptr) {
        return std::string_view("SessionGetRuntimeId");
    }
    if (std::get_if<Request::SessionGetShareTokenAccessMode>(&variant) != nullptr) {
        return std::string_view("SessionGetShareTokenAccessMode");
    }
    if (std::get_if<Request::SessionGetShareTokenInfoHash>(&variant) != nullptr) {
        return std::string_view("SessionGetShareTokenInfoHash");
    }
    if (std::get_if<Request::SessionGetShareTokenSuggestedName>(&variant) != nullptr) {
        return std::string_view("SessionGetShareTokenSuggestedName");
    }
    if (std::get_if<Request::SessionGetStateMonitor>(&variant) != nullptr) {
        return std::string_view("SessionGetStateMonitor");
    }
    if (std::get_if<Request::SessionGetStoreDir>(&variant) != nullptr) {
        return std::string_view("SessionGetStoreDir");
    }
    if (std::get_if<Request::SessionGetUserProvidedPeers>(&variant) != nullptr) {
        return std::string_view("SessionGetUserProvidedPeers");
    }
    if (std::get_if<Request::SessionInitNetwork>(&variant) != nullptr) {
        return std::string_view("SessionInitNetwork");
    }
    if (std::get_if<Request::SessionIsLocalDiscoveryEnabled>(&variant) != nullptr) {
        return std::string_view("SessionIsLocalDiscoveryEnabled");
    }
    if (std::get_if<Request::SessionIsPexRecvEnabled>(&variant) != nullptr) {
        return std::string_view("SessionIsPexRecvEnabled");
    }
    if (std::get_if<Request::SessionIsPexSendEnabled>(&variant) != nullptr) {
        return std::string_view("SessionIsPexSendEnabled");
    }
    if (std::get_if<Request::SessionIsPortForwardingEnabled>(&variant) != nullptr) {
        return std::string_view("SessionIsPortForwardingEnabled");
    }
    if (std::get_if<Request::SessionListRepositories>(&variant) != nullptr) {
        return std::string_view("SessionListRepositories");
    }
    if (std::get_if<Request::SessionMirrorExists>(&variant) != nullptr) {
        return std::string_view("SessionMirrorExists");
    }
    if (std::get_if<Request::SessionOpenRepository>(&variant) != nullptr) {
        return std::string_view("SessionOpenRepository");
    }
    if (std::get_if<Request::SessionRemoveUserProvidedPeers>(&variant) != nullptr) {
        return std::string_view("SessionRemoveUserProvidedPeers");
    }
    if (std::get_if<Request::SessionSetDefaultBlockExpiration>(&variant) != nullptr) {
        return std::string_view("SessionSetDefaultBlockExpiration");
    }
    if (std::get_if<Request::SessionSetDefaultQuota>(&variant) != nullptr) {
        return std::string_view("SessionSetDefaultQuota");
    }
    if (std::get_if<Request::SessionSetDefaultRepositoryExpiration>(&variant) != nullptr) {
        return std::string_view("SessionSetDefaultRepositoryExpiration");
    }
    if (std::get_if<Request::SessionSetLocalDiscoveryEnabled>(&variant) != nullptr) {
        return std::string_view("SessionSetLocalDiscoveryEnabled");
    }
    if (std::get_if<Request::SessionSetMountRoot>(&variant) != nullptr) {
        return std::string_view("SessionSetMountRoot");
    }
    if (std::get_if<Request::SessionSetPexRecvEnabled>(&variant) != nullptr) {
        return std::string_view("SessionSetPexRecvEnabled");
    }
    if (std::get_if<Request::SessionSetPexSendEnabled>(&variant) != nullptr) {
        return std::string_view("SessionSetPexSendEnabled");
    }
    if (std::get_if<Request::SessionSetPortForwardingEnabled>(&variant) != nullptr) {
        return std::string_view("SessionSetPortForwardingEnabled");
    }
    if (std::get_if<Request::SessionSetStoreDir>(&variant) != nullptr) {
        return std::string_view("SessionSetStoreDir");
    }
    if (std::get_if<Request::SessionSubscribeToNetwork>(&variant) != nullptr) {
        return std::string_view("SessionSubscribeToNetwork");
    }
    if (std::get_if<Request::SessionSubscribeToStateMonitor>(&variant) != nullptr) {
        return std::string_view("SessionSubscribeToStateMonitor");
    }
    if (std::get_if<Request::SessionUnsubscribe>(&variant) != nullptr) {
        return std::string_view("SessionUnsubscribe");
    }
    if (std::get_if<Request::SessionValidateShareToken>(&variant) != nullptr) {
        return std::string_view("SessionValidateShareToken");
    }
    throw std::bad_cast();
}

std::string_view variant_name(const Response::Alternatives& variant) {
    if (std::get_if<Response::AccessMode>(&variant) != nullptr) {
        return std::string_view("AccessMode");
    }
    if (std::get_if<Response::Bool>(&variant) != nullptr) {
        return std::string_view("Bool");
    }
    if (std::get_if<Response::Bytes>(&variant) != nullptr) {
        return std::string_view("Bytes");
    }
    if (std::get_if<Response::DirectoryEntries>(&variant) != nullptr) {
        return std::string_view("DirectoryEntries");
    }
    if (std::get_if<Response::Duration>(&variant) != nullptr) {
        return std::string_view("Duration");
    }
    if (std::get_if<Response::EntryType>(&variant) != nullptr) {
        return std::string_view("EntryType");
    }
    if (std::get_if<Response::File>(&variant) != nullptr) {
        return std::string_view("File");
    }
    if (std::get_if<Response::NatBehavior>(&variant) != nullptr) {
        return std::string_view("NatBehavior");
    }
    if (std::get_if<Response::NetworkEvent>(&variant) != nullptr) {
        return std::string_view("NetworkEvent");
    }
    if (std::get_if<Response::None>(&variant) != nullptr) {
        return std::string_view("None");
    }
    if (std::get_if<Response::PasswordSalt>(&variant) != nullptr) {
        return std::string_view("PasswordSalt");
    }
    if (std::get_if<Response::Path>(&variant) != nullptr) {
        return std::string_view("Path");
    }
    if (std::get_if<Response::PeerAddrs>(&variant) != nullptr) {
        return std::string_view("PeerAddrs");
    }
    if (std::get_if<Response::PeerInfos>(&variant) != nullptr) {
        return std::string_view("PeerInfos");
    }
    if (std::get_if<Response::Progress>(&variant) != nullptr) {
        return std::string_view("Progress");
    }
    if (std::get_if<Response::PublicRuntimeId>(&variant) != nullptr) {
        return std::string_view("PublicRuntimeId");
    }
    if (std::get_if<Response::QuotaInfo>(&variant) != nullptr) {
        return std::string_view("QuotaInfo");
    }
    if (std::get_if<Response::Repositories>(&variant) != nullptr) {
        return std::string_view("Repositories");
    }
    if (std::get_if<Response::Repository>(&variant) != nullptr) {
        return std::string_view("Repository");
    }
    if (std::get_if<Response::RepositoryEvent>(&variant) != nullptr) {
        return std::string_view("RepositoryEvent");
    }
    if (std::get_if<Response::SecretKey>(&variant) != nullptr) {
        return std::string_view("SecretKey");
    }
    if (std::get_if<Response::ShareToken>(&variant) != nullptr) {
        return std::string_view("ShareToken");
    }
    if (std::get_if<Response::SocketAddr>(&variant) != nullptr) {
        return std::string_view("SocketAddr");
    }
    if (std::get_if<Response::StateMonitor>(&variant) != nullptr) {
        return std::string_view("StateMonitor");
    }
    if (std::get_if<Response::StateMonitorEvent>(&variant) != nullptr) {
        return std::string_view("StateMonitorEvent");
    }
    if (std::get_if<Response::Stats>(&variant) != nullptr) {
        return std::string_view("Stats");
    }
    if (std::get_if<Response::StorageSize>(&variant) != nullptr) {
        return std::string_view("StorageSize");
    }
    if (std::get_if<Response::String>(&variant) != nullptr) {
        return std::string_view("String");
    }
    if (std::get_if<Response::U16>(&variant) != nullptr) {
        return std::string_view("U16");
    }
    if (std::get_if<Response::U64>(&variant) != nullptr) {
        return std::string_view("U64");
    }
    throw std::bad_cast();
}

} // namespace ouisync
