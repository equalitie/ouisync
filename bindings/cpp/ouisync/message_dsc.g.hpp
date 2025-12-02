// This file is auto generated. Do not edit.

#pragma once

#include <ouisync/describe.hpp>

namespace ouisync {

template<> struct describe::Struct<Request::FileClose> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::FileClose& v) {
        o.field(v.file);
    }
};

template<> struct describe::Struct<Request::FileFlush> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::FileFlush& v) {
        o.field(v.file);
    }
};

template<> struct describe::Struct<Request::FileGetLength> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::FileGetLength& v) {
        o.field(v.file);
    }
};

template<> struct describe::Struct<Request::FileGetProgress> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::FileGetProgress& v) {
        o.field(v.file);
    }
};

template<> struct describe::Struct<Request::FileRead> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::FileRead& v) {
        o.field(v.file);
        o.field(v.offset);
        o.field(v.size);
    }
};

template<> struct describe::Struct<Request::FileTruncate> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::FileTruncate& v) {
        o.field(v.file);
        o.field(v.len);
    }
};

template<> struct describe::Struct<Request::FileWrite> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::FileWrite& v) {
        o.field(v.file);
        o.field(v.offset);
        o.field(v.data);
    }
};

template<> struct describe::Struct<Request::RepositoryClose> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryClose& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryCreateDirectory> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryCreateDirectory& v) {
        o.field(v.repo);
        o.field(v.path);
    }
};

template<> struct describe::Struct<Request::RepositoryCreateFile> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryCreateFile& v) {
        o.field(v.repo);
        o.field(v.path);
    }
};

template<> struct describe::Struct<Request::RepositoryCreateMirror> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryCreateMirror& v) {
        o.field(v.repo);
        o.field(v.host);
    }
};

template<> struct describe::Struct<Request::RepositoryDelete> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryDelete& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryDeleteMirror> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryDeleteMirror& v) {
        o.field(v.repo);
        o.field(v.host);
    }
};

template<> struct describe::Struct<Request::RepositoryExport> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryExport& v) {
        o.field(v.repo);
        o.field(v.output_path);
    }
};

template<> struct describe::Struct<Request::RepositoryFileExists> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryFileExists& v) {
        o.field(v.repo);
        o.field(v.path);
    }
};

template<> struct describe::Struct<Request::RepositoryGetAccessMode> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryGetAccessMode& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryGetBlockExpiration> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryGetBlockExpiration& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryGetCredentials> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryGetCredentials& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryGetEntryType> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryGetEntryType& v) {
        o.field(v.repo);
        o.field(v.path);
    }
};

template<> struct describe::Struct<Request::RepositoryGetExpiration> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryGetExpiration& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryGetInfoHash> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryGetInfoHash& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryGetMetadata> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryGetMetadata& v) {
        o.field(v.repo);
        o.field(v.key);
    }
};

template<> struct describe::Struct<Request::RepositoryGetMountPoint> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryGetMountPoint& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryGetPath> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryGetPath& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryGetQuota> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryGetQuota& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryGetShortName> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryGetShortName& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryGetStats> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryGetStats& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryGetSyncProgress> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryGetSyncProgress& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryIsDhtEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryIsDhtEnabled& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryIsPexEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryIsPexEnabled& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryIsSyncEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryIsSyncEnabled& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryMirrorExists> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryMirrorExists& v) {
        o.field(v.repo);
        o.field(v.host);
    }
};

template<> struct describe::Struct<Request::RepositoryMount> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryMount& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryMove> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryMove& v) {
        o.field(v.repo);
        o.field(v.dst);
    }
};

template<> struct describe::Struct<Request::RepositoryMoveEntry> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryMoveEntry& v) {
        o.field(v.repo);
        o.field(v.src);
        o.field(v.dst);
    }
};

template<> struct describe::Struct<Request::RepositoryOpenFile> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryOpenFile& v) {
        o.field(v.repo);
        o.field(v.path);
    }
};

template<> struct describe::Struct<Request::RepositoryReadDirectory> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryReadDirectory& v) {
        o.field(v.repo);
        o.field(v.path);
    }
};

template<> struct describe::Struct<Request::RepositoryRemoveDirectory> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryRemoveDirectory& v) {
        o.field(v.repo);
        o.field(v.path);
        o.field(v.recursive);
    }
};

template<> struct describe::Struct<Request::RepositoryRemoveFile> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryRemoveFile& v) {
        o.field(v.repo);
        o.field(v.path);
    }
};

template<> struct describe::Struct<Request::RepositoryResetAccess> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryResetAccess& v) {
        o.field(v.repo);
        o.field(v.token);
    }
};

template<> struct describe::Struct<Request::RepositorySetAccess> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositorySetAccess& v) {
        o.field(v.repo);
        o.field(v.read);
        o.field(v.write);
    }
};

template<> struct describe::Struct<Request::RepositorySetAccessMode> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositorySetAccessMode& v) {
        o.field(v.repo);
        o.field(v.access_mode);
        o.field(v.local_secret);
    }
};

template<> struct describe::Struct<Request::RepositorySetBlockExpiration> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositorySetBlockExpiration& v) {
        o.field(v.repo);
        o.field(v.value);
    }
};

template<> struct describe::Struct<Request::RepositorySetCredentials> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositorySetCredentials& v) {
        o.field(v.repo);
        o.field(v.credentials);
    }
};

template<> struct describe::Struct<Request::RepositorySetDhtEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositorySetDhtEnabled& v) {
        o.field(v.repo);
        o.field(v.enabled);
    }
};

template<> struct describe::Struct<Request::RepositorySetExpiration> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositorySetExpiration& v) {
        o.field(v.repo);
        o.field(v.value);
    }
};

template<> struct describe::Struct<Request::RepositorySetMetadata> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositorySetMetadata& v) {
        o.field(v.repo);
        o.field(v.edits);
    }
};

template<> struct describe::Struct<Request::RepositorySetPexEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositorySetPexEnabled& v) {
        o.field(v.repo);
        o.field(v.enabled);
    }
};

template<> struct describe::Struct<Request::RepositorySetQuota> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositorySetQuota& v) {
        o.field(v.repo);
        o.field(v.value);
    }
};

template<> struct describe::Struct<Request::RepositorySetSyncEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositorySetSyncEnabled& v) {
        o.field(v.repo);
        o.field(v.enabled);
    }
};

template<> struct describe::Struct<Request::RepositoryShare> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryShare& v) {
        o.field(v.repo);
        o.field(v.access_mode);
        o.field(v.local_secret);
    }
};

template<> struct describe::Struct<Request::RepositorySubscribe> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositorySubscribe& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::RepositoryUnmount> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::RepositoryUnmount& v) {
        o.field(v.repo);
    }
};

template<> struct describe::Struct<Request::SessionAddUserProvidedPeers> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionAddUserProvidedPeers& v) {
        o.field(v.addrs);
    }
};

template<> struct describe::Struct<Request::SessionBindMetrics> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionBindMetrics& v) {
        o.field(v.addr);
    }
};

template<> struct describe::Struct<Request::SessionBindNetwork> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionBindNetwork& v) {
        o.field(v.addrs);
    }
};

template<> struct describe::Struct<Request::SessionBindRemoteControl> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionBindRemoteControl& v) {
        o.field(v.addr);
    }
};

template<> struct describe::Struct<Request::SessionCreateRepository> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionCreateRepository& v) {
        o.field(v.path);
        o.field(v.read_secret);
        o.field(v.write_secret);
        o.field(v.token);
        o.field(v.sync_enabled);
        o.field(v.dht_enabled);
        o.field(v.pex_enabled);
    }
};

template<> struct describe::Struct<Request::SessionDeleteRepositoryByName> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionDeleteRepositoryByName& v) {
        o.field(v.name);
    }
};

template<> struct describe::Struct<Request::SessionDeriveSecretKey> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionDeriveSecretKey& v) {
        o.field(v.password);
        o.field(v.salt);
    }
};

template<> struct describe::Struct<Request::SessionFindRepository> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionFindRepository& v) {
        o.field(v.name);
    }
};

template<> struct describe::Struct<Request::SessionGeneratePasswordSalt> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGeneratePasswordSalt&) {
    }
};

template<> struct describe::Struct<Request::SessionGenerateSecretKey> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGenerateSecretKey&) {
    }
};

template<> struct describe::Struct<Request::SessionGetCurrentProtocolVersion> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetCurrentProtocolVersion&) {
    }
};

template<> struct describe::Struct<Request::SessionGetDefaultBlockExpiration> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetDefaultBlockExpiration&) {
    }
};

template<> struct describe::Struct<Request::SessionGetDefaultQuota> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetDefaultQuota&) {
    }
};

template<> struct describe::Struct<Request::SessionGetDefaultRepositoryExpiration> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetDefaultRepositoryExpiration&) {
    }
};

template<> struct describe::Struct<Request::SessionGetExternalAddrV4> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetExternalAddrV4&) {
    }
};

template<> struct describe::Struct<Request::SessionGetExternalAddrV6> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetExternalAddrV6&) {
    }
};

template<> struct describe::Struct<Request::SessionGetHighestSeenProtocolVersion> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetHighestSeenProtocolVersion&) {
    }
};

template<> struct describe::Struct<Request::SessionGetLocalListenerAddrs> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetLocalListenerAddrs&) {
    }
};

template<> struct describe::Struct<Request::SessionGetMetricsListenerAddr> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetMetricsListenerAddr&) {
    }
};

template<> struct describe::Struct<Request::SessionGetMountRoot> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetMountRoot&) {
    }
};

template<> struct describe::Struct<Request::SessionGetNatBehavior> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetNatBehavior&) {
    }
};

template<> struct describe::Struct<Request::SessionGetNetworkStats> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetNetworkStats&) {
    }
};

template<> struct describe::Struct<Request::SessionGetPeers> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetPeers&) {
    }
};

template<> struct describe::Struct<Request::SessionGetRemoteControlListenerAddr> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetRemoteControlListenerAddr&) {
    }
};

template<> struct describe::Struct<Request::SessionGetRemoteListenerAddrs> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionGetRemoteListenerAddrs& v) {
        o.field(v.host);
    }
};

template<> struct describe::Struct<Request::SessionGetRuntimeId> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetRuntimeId&) {
    }
};

template<> struct describe::Struct<Request::SessionGetShareTokenAccessMode> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionGetShareTokenAccessMode& v) {
        o.field(v.token);
    }
};

template<> struct describe::Struct<Request::SessionGetShareTokenInfoHash> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionGetShareTokenInfoHash& v) {
        o.field(v.token);
    }
};

template<> struct describe::Struct<Request::SessionGetShareTokenSuggestedName> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionGetShareTokenSuggestedName& v) {
        o.field(v.token);
    }
};

template<> struct describe::Struct<Request::SessionGetStateMonitor> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionGetStateMonitor& v) {
        o.field(v.path);
    }
};

template<> struct describe::Struct<Request::SessionGetStoreDirs> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetStoreDirs&) {
    }
};

template<> struct describe::Struct<Request::SessionGetUserProvidedPeers> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionGetUserProvidedPeers&) {
    }
};

template<> struct describe::Struct<Request::SessionInitNetwork> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionInitNetwork& v) {
        o.field(v.defaults);
    }
};

template<> struct describe::Struct<Request::SessionInsertStoreDirs> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionInsertStoreDirs& v) {
        o.field(v.paths);
    }
};

template<> struct describe::Struct<Request::SessionIsLocalDiscoveryEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionIsLocalDiscoveryEnabled&) {
    }
};

template<> struct describe::Struct<Request::SessionIsPexRecvEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionIsPexRecvEnabled&) {
    }
};

template<> struct describe::Struct<Request::SessionIsPexSendEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionIsPexSendEnabled&) {
    }
};

template<> struct describe::Struct<Request::SessionIsPortForwardingEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionIsPortForwardingEnabled&) {
    }
};

template<> struct describe::Struct<Request::SessionListRepositories> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionListRepositories&) {
    }
};

template<> struct describe::Struct<Request::SessionMirrorExists> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionMirrorExists& v) {
        o.field(v.token);
        o.field(v.host);
    }
};

template<> struct describe::Struct<Request::SessionOpenRepository> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionOpenRepository& v) {
        o.field(v.path);
        o.field(v.local_secret);
    }
};

template<> struct describe::Struct<Request::SessionRemoveStoreDirs> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionRemoveStoreDirs& v) {
        o.field(v.paths);
    }
};

template<> struct describe::Struct<Request::SessionRemoveUserProvidedPeers> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionRemoveUserProvidedPeers& v) {
        o.field(v.addrs);
    }
};

template<> struct describe::Struct<Request::SessionSetDefaultBlockExpiration> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionSetDefaultBlockExpiration& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Request::SessionSetDefaultQuota> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionSetDefaultQuota& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Request::SessionSetDefaultRepositoryExpiration> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionSetDefaultRepositoryExpiration& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Request::SessionSetLocalDiscoveryEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionSetLocalDiscoveryEnabled& v) {
        o.field(v.enabled);
    }
};

template<> struct describe::Struct<Request::SessionSetMountRoot> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionSetMountRoot& v) {
        o.field(v.path);
    }
};

template<> struct describe::Struct<Request::SessionSetPexRecvEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionSetPexRecvEnabled& v) {
        o.field(v.enabled);
    }
};

template<> struct describe::Struct<Request::SessionSetPexSendEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionSetPexSendEnabled& v) {
        o.field(v.enabled);
    }
};

template<> struct describe::Struct<Request::SessionSetPortForwardingEnabled> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionSetPortForwardingEnabled& v) {
        o.field(v.enabled);
    }
};

template<> struct describe::Struct<Request::SessionSetStoreDirs> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionSetStoreDirs& v) {
        o.field(v.paths);
    }
};

template<> struct describe::Struct<Request::SessionSubscribeToNetwork> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Request::SessionSubscribeToNetwork&) {
    }
};

template<> struct describe::Struct<Request::SessionSubscribeToStateMonitor> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionSubscribeToStateMonitor& v) {
        o.field(v.path);
    }
};

template<> struct describe::Struct<Request::SessionUnsubscribe> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionUnsubscribe& v) {
        o.field(v.id);
    }
};

template<> struct describe::Struct<Request::SessionValidateShareToken> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Request::SessionValidateShareToken& v) {
        o.field(v.token);
    }
};

template<> struct describe::Struct<Request> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Request& v) {
        o.field(v.value);
    }
};

template<> struct VariantBuilder<Request::Alternatives> {
    template<class AltBuilder>
    static Request::Alternatives build(std::string_view name, const AltBuilder& builder) {
        if (name == "FileClose") {
            return builder.template build<Request::FileClose>();
        }
        if (name == "FileFlush") {
            return builder.template build<Request::FileFlush>();
        }
        if (name == "FileGetLength") {
            return builder.template build<Request::FileGetLength>();
        }
        if (name == "FileGetProgress") {
            return builder.template build<Request::FileGetProgress>();
        }
        if (name == "FileRead") {
            return builder.template build<Request::FileRead>();
        }
        if (name == "FileTruncate") {
            return builder.template build<Request::FileTruncate>();
        }
        if (name == "FileWrite") {
            return builder.template build<Request::FileWrite>();
        }
        if (name == "RepositoryClose") {
            return builder.template build<Request::RepositoryClose>();
        }
        if (name == "RepositoryCreateDirectory") {
            return builder.template build<Request::RepositoryCreateDirectory>();
        }
        if (name == "RepositoryCreateFile") {
            return builder.template build<Request::RepositoryCreateFile>();
        }
        if (name == "RepositoryCreateMirror") {
            return builder.template build<Request::RepositoryCreateMirror>();
        }
        if (name == "RepositoryDelete") {
            return builder.template build<Request::RepositoryDelete>();
        }
        if (name == "RepositoryDeleteMirror") {
            return builder.template build<Request::RepositoryDeleteMirror>();
        }
        if (name == "RepositoryExport") {
            return builder.template build<Request::RepositoryExport>();
        }
        if (name == "RepositoryFileExists") {
            return builder.template build<Request::RepositoryFileExists>();
        }
        if (name == "RepositoryGetAccessMode") {
            return builder.template build<Request::RepositoryGetAccessMode>();
        }
        if (name == "RepositoryGetBlockExpiration") {
            return builder.template build<Request::RepositoryGetBlockExpiration>();
        }
        if (name == "RepositoryGetCredentials") {
            return builder.template build<Request::RepositoryGetCredentials>();
        }
        if (name == "RepositoryGetEntryType") {
            return builder.template build<Request::RepositoryGetEntryType>();
        }
        if (name == "RepositoryGetExpiration") {
            return builder.template build<Request::RepositoryGetExpiration>();
        }
        if (name == "RepositoryGetInfoHash") {
            return builder.template build<Request::RepositoryGetInfoHash>();
        }
        if (name == "RepositoryGetMetadata") {
            return builder.template build<Request::RepositoryGetMetadata>();
        }
        if (name == "RepositoryGetMountPoint") {
            return builder.template build<Request::RepositoryGetMountPoint>();
        }
        if (name == "RepositoryGetPath") {
            return builder.template build<Request::RepositoryGetPath>();
        }
        if (name == "RepositoryGetQuota") {
            return builder.template build<Request::RepositoryGetQuota>();
        }
        if (name == "RepositoryGetShortName") {
            return builder.template build<Request::RepositoryGetShortName>();
        }
        if (name == "RepositoryGetStats") {
            return builder.template build<Request::RepositoryGetStats>();
        }
        if (name == "RepositoryGetSyncProgress") {
            return builder.template build<Request::RepositoryGetSyncProgress>();
        }
        if (name == "RepositoryIsDhtEnabled") {
            return builder.template build<Request::RepositoryIsDhtEnabled>();
        }
        if (name == "RepositoryIsPexEnabled") {
            return builder.template build<Request::RepositoryIsPexEnabled>();
        }
        if (name == "RepositoryIsSyncEnabled") {
            return builder.template build<Request::RepositoryIsSyncEnabled>();
        }
        if (name == "RepositoryMirrorExists") {
            return builder.template build<Request::RepositoryMirrorExists>();
        }
        if (name == "RepositoryMount") {
            return builder.template build<Request::RepositoryMount>();
        }
        if (name == "RepositoryMove") {
            return builder.template build<Request::RepositoryMove>();
        }
        if (name == "RepositoryMoveEntry") {
            return builder.template build<Request::RepositoryMoveEntry>();
        }
        if (name == "RepositoryOpenFile") {
            return builder.template build<Request::RepositoryOpenFile>();
        }
        if (name == "RepositoryReadDirectory") {
            return builder.template build<Request::RepositoryReadDirectory>();
        }
        if (name == "RepositoryRemoveDirectory") {
            return builder.template build<Request::RepositoryRemoveDirectory>();
        }
        if (name == "RepositoryRemoveFile") {
            return builder.template build<Request::RepositoryRemoveFile>();
        }
        if (name == "RepositoryResetAccess") {
            return builder.template build<Request::RepositoryResetAccess>();
        }
        if (name == "RepositorySetAccess") {
            return builder.template build<Request::RepositorySetAccess>();
        }
        if (name == "RepositorySetAccessMode") {
            return builder.template build<Request::RepositorySetAccessMode>();
        }
        if (name == "RepositorySetBlockExpiration") {
            return builder.template build<Request::RepositorySetBlockExpiration>();
        }
        if (name == "RepositorySetCredentials") {
            return builder.template build<Request::RepositorySetCredentials>();
        }
        if (name == "RepositorySetDhtEnabled") {
            return builder.template build<Request::RepositorySetDhtEnabled>();
        }
        if (name == "RepositorySetExpiration") {
            return builder.template build<Request::RepositorySetExpiration>();
        }
        if (name == "RepositorySetMetadata") {
            return builder.template build<Request::RepositorySetMetadata>();
        }
        if (name == "RepositorySetPexEnabled") {
            return builder.template build<Request::RepositorySetPexEnabled>();
        }
        if (name == "RepositorySetQuota") {
            return builder.template build<Request::RepositorySetQuota>();
        }
        if (name == "RepositorySetSyncEnabled") {
            return builder.template build<Request::RepositorySetSyncEnabled>();
        }
        if (name == "RepositoryShare") {
            return builder.template build<Request::RepositoryShare>();
        }
        if (name == "RepositorySubscribe") {
            return builder.template build<Request::RepositorySubscribe>();
        }
        if (name == "RepositoryUnmount") {
            return builder.template build<Request::RepositoryUnmount>();
        }
        if (name == "SessionAddUserProvidedPeers") {
            return builder.template build<Request::SessionAddUserProvidedPeers>();
        }
        if (name == "SessionBindMetrics") {
            return builder.template build<Request::SessionBindMetrics>();
        }
        if (name == "SessionBindNetwork") {
            return builder.template build<Request::SessionBindNetwork>();
        }
        if (name == "SessionBindRemoteControl") {
            return builder.template build<Request::SessionBindRemoteControl>();
        }
        if (name == "SessionCreateRepository") {
            return builder.template build<Request::SessionCreateRepository>();
        }
        if (name == "SessionDeleteRepositoryByName") {
            return builder.template build<Request::SessionDeleteRepositoryByName>();
        }
        if (name == "SessionDeriveSecretKey") {
            return builder.template build<Request::SessionDeriveSecretKey>();
        }
        if (name == "SessionFindRepository") {
            return builder.template build<Request::SessionFindRepository>();
        }
        if (name == "SessionGeneratePasswordSalt") {
            return builder.template build<Request::SessionGeneratePasswordSalt>();
        }
        if (name == "SessionGenerateSecretKey") {
            return builder.template build<Request::SessionGenerateSecretKey>();
        }
        if (name == "SessionGetCurrentProtocolVersion") {
            return builder.template build<Request::SessionGetCurrentProtocolVersion>();
        }
        if (name == "SessionGetDefaultBlockExpiration") {
            return builder.template build<Request::SessionGetDefaultBlockExpiration>();
        }
        if (name == "SessionGetDefaultQuota") {
            return builder.template build<Request::SessionGetDefaultQuota>();
        }
        if (name == "SessionGetDefaultRepositoryExpiration") {
            return builder.template build<Request::SessionGetDefaultRepositoryExpiration>();
        }
        if (name == "SessionGetExternalAddrV4") {
            return builder.template build<Request::SessionGetExternalAddrV4>();
        }
        if (name == "SessionGetExternalAddrV6") {
            return builder.template build<Request::SessionGetExternalAddrV6>();
        }
        if (name == "SessionGetHighestSeenProtocolVersion") {
            return builder.template build<Request::SessionGetHighestSeenProtocolVersion>();
        }
        if (name == "SessionGetLocalListenerAddrs") {
            return builder.template build<Request::SessionGetLocalListenerAddrs>();
        }
        if (name == "SessionGetMetricsListenerAddr") {
            return builder.template build<Request::SessionGetMetricsListenerAddr>();
        }
        if (name == "SessionGetMountRoot") {
            return builder.template build<Request::SessionGetMountRoot>();
        }
        if (name == "SessionGetNatBehavior") {
            return builder.template build<Request::SessionGetNatBehavior>();
        }
        if (name == "SessionGetNetworkStats") {
            return builder.template build<Request::SessionGetNetworkStats>();
        }
        if (name == "SessionGetPeers") {
            return builder.template build<Request::SessionGetPeers>();
        }
        if (name == "SessionGetRemoteControlListenerAddr") {
            return builder.template build<Request::SessionGetRemoteControlListenerAddr>();
        }
        if (name == "SessionGetRemoteListenerAddrs") {
            return builder.template build<Request::SessionGetRemoteListenerAddrs>();
        }
        if (name == "SessionGetRuntimeId") {
            return builder.template build<Request::SessionGetRuntimeId>();
        }
        if (name == "SessionGetShareTokenAccessMode") {
            return builder.template build<Request::SessionGetShareTokenAccessMode>();
        }
        if (name == "SessionGetShareTokenInfoHash") {
            return builder.template build<Request::SessionGetShareTokenInfoHash>();
        }
        if (name == "SessionGetShareTokenSuggestedName") {
            return builder.template build<Request::SessionGetShareTokenSuggestedName>();
        }
        if (name == "SessionGetStateMonitor") {
            return builder.template build<Request::SessionGetStateMonitor>();
        }
        if (name == "SessionGetStoreDirs") {
            return builder.template build<Request::SessionGetStoreDirs>();
        }
        if (name == "SessionGetUserProvidedPeers") {
            return builder.template build<Request::SessionGetUserProvidedPeers>();
        }
        if (name == "SessionInitNetwork") {
            return builder.template build<Request::SessionInitNetwork>();
        }
        if (name == "SessionInsertStoreDirs") {
            return builder.template build<Request::SessionInsertStoreDirs>();
        }
        if (name == "SessionIsLocalDiscoveryEnabled") {
            return builder.template build<Request::SessionIsLocalDiscoveryEnabled>();
        }
        if (name == "SessionIsPexRecvEnabled") {
            return builder.template build<Request::SessionIsPexRecvEnabled>();
        }
        if (name == "SessionIsPexSendEnabled") {
            return builder.template build<Request::SessionIsPexSendEnabled>();
        }
        if (name == "SessionIsPortForwardingEnabled") {
            return builder.template build<Request::SessionIsPortForwardingEnabled>();
        }
        if (name == "SessionListRepositories") {
            return builder.template build<Request::SessionListRepositories>();
        }
        if (name == "SessionMirrorExists") {
            return builder.template build<Request::SessionMirrorExists>();
        }
        if (name == "SessionOpenRepository") {
            return builder.template build<Request::SessionOpenRepository>();
        }
        if (name == "SessionRemoveStoreDirs") {
            return builder.template build<Request::SessionRemoveStoreDirs>();
        }
        if (name == "SessionRemoveUserProvidedPeers") {
            return builder.template build<Request::SessionRemoveUserProvidedPeers>();
        }
        if (name == "SessionSetDefaultBlockExpiration") {
            return builder.template build<Request::SessionSetDefaultBlockExpiration>();
        }
        if (name == "SessionSetDefaultQuota") {
            return builder.template build<Request::SessionSetDefaultQuota>();
        }
        if (name == "SessionSetDefaultRepositoryExpiration") {
            return builder.template build<Request::SessionSetDefaultRepositoryExpiration>();
        }
        if (name == "SessionSetLocalDiscoveryEnabled") {
            return builder.template build<Request::SessionSetLocalDiscoveryEnabled>();
        }
        if (name == "SessionSetMountRoot") {
            return builder.template build<Request::SessionSetMountRoot>();
        }
        if (name == "SessionSetPexRecvEnabled") {
            return builder.template build<Request::SessionSetPexRecvEnabled>();
        }
        if (name == "SessionSetPexSendEnabled") {
            return builder.template build<Request::SessionSetPexSendEnabled>();
        }
        if (name == "SessionSetPortForwardingEnabled") {
            return builder.template build<Request::SessionSetPortForwardingEnabled>();
        }
        if (name == "SessionSetStoreDirs") {
            return builder.template build<Request::SessionSetStoreDirs>();
        }
        if (name == "SessionSubscribeToNetwork") {
            return builder.template build<Request::SessionSubscribeToNetwork>();
        }
        if (name == "SessionSubscribeToStateMonitor") {
            return builder.template build<Request::SessionSubscribeToStateMonitor>();
        }
        if (name == "SessionUnsubscribe") {
            return builder.template build<Request::SessionUnsubscribe>();
        }
        if (name == "SessionValidateShareToken") {
            return builder.template build<Request::SessionValidateShareToken>();
        }

        throw std::runtime_error("invalid variant name for Request");
    }
};

std::string_view variant_name(const Request::Alternatives& variant);

template<> struct describe::Struct<Response::AccessMode> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::AccessMode& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::Bool> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::Bool& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::Bytes> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::Bytes& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::DirectoryEntries> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::DirectoryEntries& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::Duration> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::Duration& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::EntryType> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::EntryType& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::File> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::File& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::NatBehavior> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::NatBehavior& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::NetworkEvent> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::NetworkEvent& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::None> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Response::None&) {
    }
};

template<> struct describe::Struct<Response::PasswordSalt> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::PasswordSalt& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::Path> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::Path& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::Paths> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::Paths& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::PeerAddrs> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::PeerAddrs& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::PeerInfos> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::PeerInfos& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::Progress> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::Progress& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::PublicRuntimeId> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::PublicRuntimeId& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::QuotaInfo> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::QuotaInfo& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::Repositories> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::Repositories& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::Repository> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::Repository& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::RepositoryEvent> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Response::RepositoryEvent&) {
    }
};

template<> struct describe::Struct<Response::SecretKey> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::SecretKey& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::ShareToken> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::ShareToken& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::SocketAddr> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::SocketAddr& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::StateMonitor> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::StateMonitor& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::StateMonitorEvent> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer&, Response::StateMonitorEvent&) {
    }
};

template<> struct describe::Struct<Response::Stats> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::Stats& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::StorageSize> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::StorageSize& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::String> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::String& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::U16> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::U16& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response::U64> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response::U64& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Response> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Response& v) {
        o.field(v.value);
    }
};

template<> struct VariantBuilder<Response::Alternatives> {
    template<class AltBuilder>
    static Response::Alternatives build(std::string_view name, const AltBuilder& builder) {
        if (name == "AccessMode") {
            return builder.template build<Response::AccessMode>();
        }
        if (name == "Bool") {
            return builder.template build<Response::Bool>();
        }
        if (name == "Bytes") {
            return builder.template build<Response::Bytes>();
        }
        if (name == "DirectoryEntries") {
            return builder.template build<Response::DirectoryEntries>();
        }
        if (name == "Duration") {
            return builder.template build<Response::Duration>();
        }
        if (name == "EntryType") {
            return builder.template build<Response::EntryType>();
        }
        if (name == "File") {
            return builder.template build<Response::File>();
        }
        if (name == "NatBehavior") {
            return builder.template build<Response::NatBehavior>();
        }
        if (name == "NetworkEvent") {
            return builder.template build<Response::NetworkEvent>();
        }
        if (name == "None") {
            return builder.template build<Response::None>();
        }
        if (name == "PasswordSalt") {
            return builder.template build<Response::PasswordSalt>();
        }
        if (name == "Path") {
            return builder.template build<Response::Path>();
        }
        if (name == "Paths") {
            return builder.template build<Response::Paths>();
        }
        if (name == "PeerAddrs") {
            return builder.template build<Response::PeerAddrs>();
        }
        if (name == "PeerInfos") {
            return builder.template build<Response::PeerInfos>();
        }
        if (name == "Progress") {
            return builder.template build<Response::Progress>();
        }
        if (name == "PublicRuntimeId") {
            return builder.template build<Response::PublicRuntimeId>();
        }
        if (name == "QuotaInfo") {
            return builder.template build<Response::QuotaInfo>();
        }
        if (name == "Repositories") {
            return builder.template build<Response::Repositories>();
        }
        if (name == "Repository") {
            return builder.template build<Response::Repository>();
        }
        if (name == "RepositoryEvent") {
            return builder.template build<Response::RepositoryEvent>();
        }
        if (name == "SecretKey") {
            return builder.template build<Response::SecretKey>();
        }
        if (name == "ShareToken") {
            return builder.template build<Response::ShareToken>();
        }
        if (name == "SocketAddr") {
            return builder.template build<Response::SocketAddr>();
        }
        if (name == "StateMonitor") {
            return builder.template build<Response::StateMonitor>();
        }
        if (name == "StateMonitorEvent") {
            return builder.template build<Response::StateMonitorEvent>();
        }
        if (name == "Stats") {
            return builder.template build<Response::Stats>();
        }
        if (name == "StorageSize") {
            return builder.template build<Response::StorageSize>();
        }
        if (name == "String") {
            return builder.template build<Response::String>();
        }
        if (name == "U16") {
            return builder.template build<Response::U16>();
        }
        if (name == "U64") {
            return builder.template build<Response::U64>();
        }

        throw std::runtime_error("invalid variant name for Response");
    }
};

std::string_view variant_name(const Response::Alternatives& variant);

} // namespace ouisync
