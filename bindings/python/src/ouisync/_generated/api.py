from __future__ import annotations

import typing
from dataclasses import dataclass
from enum import IntEnum
from typing import ClassVar

@dataclass
class SecretKey:
    _shape: ClassVar[str] = "unnamed"
    value: bytes
    def __repr__(self) -> str:
        return f"{type(self).__name__}(******)"

@dataclass
class Password:
    _shape: ClassVar[str] = "unnamed"
    value: str
    def __repr__(self) -> str:
        return f"{type(self).__name__}(******)"

@dataclass
class PasswordSalt:
    _shape: ClassVar[str] = "unnamed"
    value: bytes

@dataclass
class StorageSize:
    _shape: ClassVar[str] = "named"
    bytes: int

class AccessMode(IntEnum):
    BLIND = 0
    READ = 1
    WRITE = 2

class LocalSecret:
    _variants: ClassVar[dict[str, type]] = {}

@dataclass
class LocalSecret_Password(LocalSecret):
    _tag: ClassVar[str] = "Password"
    _shape: ClassVar[str] = "unnamed"
    value: Password

@dataclass
class LocalSecret_SecretKey(LocalSecret):
    _tag: ClassVar[str] = "SecretKey"
    _shape: ClassVar[str] = "unnamed"
    value: SecretKey

LocalSecret._variants = {
    "Password": LocalSecret_Password,
    "SecretKey": LocalSecret_SecretKey,
}

class SetLocalSecret:
    _variants: ClassVar[dict[str, type]] = {}

@dataclass
class SetLocalSecret_Password(SetLocalSecret):
    _tag: ClassVar[str] = "Password"
    _shape: ClassVar[str] = "unnamed"
    value: Password

@dataclass
class SetLocalSecret_KeyAndSalt(SetLocalSecret):
    _tag: ClassVar[str] = "KeyAndSalt"
    _shape: ClassVar[str] = "named"
    key: SecretKey
    salt: PasswordSalt

SetLocalSecret._variants = {
    "Password": SetLocalSecret_Password,
    "KeyAndSalt": SetLocalSecret_KeyAndSalt,
}

@dataclass
class ShareToken:
    _shape: ClassVar[str] = "unnamed"
    value: str

class AccessChange:
    _variants: ClassVar[dict[str, type]] = {}

@dataclass
class AccessChange_Enable(AccessChange):
    _tag: ClassVar[str] = "Enable"
    _shape: ClassVar[str] = "unnamed"
    value: SetLocalSecret | None

@dataclass
class AccessChange_Disable(AccessChange):
    _tag: ClassVar[str] = "Disable"
    _shape: ClassVar[str] = "unit"

AccessChange._variants = {
    "Enable": AccessChange_Enable,
    "Disable": AccessChange_Disable,
}

class EntryType(IntEnum):
    FILE = 1
    DIRECTORY = 2

class NetworkEvent(IntEnum):
    PROTOCOL_VERSION_MISMATCH = 0
    PEER_SET_CHANGE = 1

@dataclass
class PeerInfo:
    _shape: ClassVar[str] = "named"
    addr: str
    source: PeerSource
    state: PeerState
    stats: Stats

class PeerSource(IntEnum):
    USER_PROVIDED = 0
    LISTENER = 1
    LOCAL_DISCOVERY = 2
    DHT = 3
    PEER_EXCHANGE = 4

class PeerState:
    _variants: ClassVar[dict[str, type]] = {}

@dataclass
class PeerState_Known(PeerState):
    _tag: ClassVar[str] = "Known"
    _shape: ClassVar[str] = "unit"

@dataclass
class PeerState_Connecting(PeerState):
    _tag: ClassVar[str] = "Connecting"
    _shape: ClassVar[str] = "unit"

@dataclass
class PeerState_Handshaking(PeerState):
    _tag: ClassVar[str] = "Handshaking"
    _shape: ClassVar[str] = "unit"

@dataclass
class PeerState_Active(PeerState):
    _tag: ClassVar[str] = "Active"
    _shape: ClassVar[str] = "named"
    id: PublicRuntimeId
    since: typing.Any

PeerState._variants = {
    "Known": PeerState_Known,
    "Connecting": PeerState_Connecting,
    "Handshaking": PeerState_Handshaking,
    "Active": PeerState_Active,
}

@dataclass
class PublicRuntimeId:
    _shape: ClassVar[str] = "unnamed"
    value: bytes

@dataclass
class Stats:
    _shape: ClassVar[str] = "named"
    bytes_tx: int
    bytes_rx: int
    throughput_tx: int
    throughput_rx: int

@dataclass
class Progress:
    _shape: ClassVar[str] = "named"
    value: int
    total: int

@dataclass
class TopicId:
    _shape: ClassVar[str] = "unnamed"
    value: bytes

class NatBehavior(IntEnum):
    ENDPOINT_INDEPENDENT = 0
    ADDRESS_DEPENDENT = 1
    ADDRESS_AND_PORT_DEPENDENT = 2

class ErrorCode(IntEnum):
    OK = 0
    PERMISSION_DENIED = 1
    INVALID_INPUT = 2
    INVALID_DATA = 3
    ALREADY_EXISTS = 4
    NOT_FOUND = 5
    AMBIGUOUS = 6
    UNSUPPORTED = 8
    INTERRUPTED = 9
    CONNECTION_REFUSED = 1025
    CONNECTION_ABORTED = 1026
    TRANSPORT_ERROR = 1027
    LISTENER_BIND_ERROR = 1028
    LISTENER_ACCEPT_ERROR = 1029
    STORE_ERROR = 2049
    IS_DIRECTORY = 2050
    NOT_DIRECTORY = 2051
    DIRECTORY_NOT_EMPTY = 2052
    RESOURCE_BUSY = 2053
    RUNTIME_INITIALIZE_ERROR = 4097
    CONFIG_ERROR = 4099
    TLS_CERTIFICATES_NOT_FOUND = 4100
    TLS_CERTIFICATES_INVALID = 4101
    TLS_KEYS_NOT_FOUND = 4102
    TLS_CONFIG_ERROR = 4103
    VFS_DRIVER_INSTALL_ERROR = 4104
    VFS_OTHER_ERROR = 4105
    SERVICE_ALREADY_RUNNING = 4106
    STORE_DIR_UNSPECIFIED = 4107
    MOUNT_DIR_UNSPECIFIED = 4108
    OTHER = 65535

class OuisyncError(Exception):
    def __init__(self, code: "ErrorCode", message: str | None = None, sources: list[str] | None = None):
        self.code = code
        self.message = message
        self.sources = sources or []
        super().__init__(message or str(code))

class OuisyncError_PermissionDenied(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.PERMISSION_DENIED, message, sources)

class OuisyncError_InvalidInput(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.INVALID_INPUT, message, sources)

class OuisyncError_InvalidData(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.INVALID_DATA, message, sources)

class OuisyncError_AlreadyExists(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.ALREADY_EXISTS, message, sources)

class OuisyncError_NotFound(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.NOT_FOUND, message, sources)

class OuisyncError_Ambiguous(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.AMBIGUOUS, message, sources)

class OuisyncError_Unsupported(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.UNSUPPORTED, message, sources)

class OuisyncError_Interrupted(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.INTERRUPTED, message, sources)

class OuisyncError_ConnectionRefused(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.CONNECTION_REFUSED, message, sources)

class OuisyncError_ConnectionAborted(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.CONNECTION_ABORTED, message, sources)

class OuisyncError_TransportError(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.TRANSPORT_ERROR, message, sources)

class OuisyncError_ListenerBindError(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.LISTENER_BIND_ERROR, message, sources)

class OuisyncError_ListenerAcceptError(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.LISTENER_ACCEPT_ERROR, message, sources)

class OuisyncError_StoreError(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.STORE_ERROR, message, sources)

class OuisyncError_IsDirectory(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.IS_DIRECTORY, message, sources)

class OuisyncError_NotDirectory(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.NOT_DIRECTORY, message, sources)

class OuisyncError_DirectoryNotEmpty(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.DIRECTORY_NOT_EMPTY, message, sources)

class OuisyncError_ResourceBusy(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.RESOURCE_BUSY, message, sources)

class OuisyncError_RuntimeInitializeError(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.RUNTIME_INITIALIZE_ERROR, message, sources)

class OuisyncError_ConfigError(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.CONFIG_ERROR, message, sources)

class OuisyncError_TlsCertificatesNotFound(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.TLS_CERTIFICATES_NOT_FOUND, message, sources)

class OuisyncError_TlsCertificatesInvalid(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.TLS_CERTIFICATES_INVALID, message, sources)

class OuisyncError_TlsKeysNotFound(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.TLS_KEYS_NOT_FOUND, message, sources)

class OuisyncError_TlsConfigError(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.TLS_CONFIG_ERROR, message, sources)

class OuisyncError_VfsDriverInstallError(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.VFS_DRIVER_INSTALL_ERROR, message, sources)

class OuisyncError_VfsOtherError(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.VFS_OTHER_ERROR, message, sources)

class OuisyncError_ServiceAlreadyRunning(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.SERVICE_ALREADY_RUNNING, message, sources)

class OuisyncError_StoreDirUnspecified(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.STORE_DIR_UNSPECIFIED, message, sources)

class OuisyncError_MountDirUnspecified(OuisyncError):
    def __init__(self, message: str | None = None, sources: list[str] | None = None):
        super().__init__(ErrorCode.MOUNT_DIR_UNSPECIFIED, message, sources)

def dispatch_error(code: "ErrorCode", message: str | None = None, sources: list[str] | None = None) -> OuisyncError:
    variant = _ERROR_VARIANTS.get(code)
    if variant is not None:
        return variant(message, sources)
    return OuisyncError(code, message, sources)

_ERROR_VARIANTS: dict[ErrorCode, type] = {
    ErrorCode.PERMISSION_DENIED: OuisyncError_PermissionDenied,
    ErrorCode.INVALID_INPUT: OuisyncError_InvalidInput,
    ErrorCode.INVALID_DATA: OuisyncError_InvalidData,
    ErrorCode.ALREADY_EXISTS: OuisyncError_AlreadyExists,
    ErrorCode.NOT_FOUND: OuisyncError_NotFound,
    ErrorCode.AMBIGUOUS: OuisyncError_Ambiguous,
    ErrorCode.UNSUPPORTED: OuisyncError_Unsupported,
    ErrorCode.INTERRUPTED: OuisyncError_Interrupted,
    ErrorCode.CONNECTION_REFUSED: OuisyncError_ConnectionRefused,
    ErrorCode.CONNECTION_ABORTED: OuisyncError_ConnectionAborted,
    ErrorCode.TRANSPORT_ERROR: OuisyncError_TransportError,
    ErrorCode.LISTENER_BIND_ERROR: OuisyncError_ListenerBindError,
    ErrorCode.LISTENER_ACCEPT_ERROR: OuisyncError_ListenerAcceptError,
    ErrorCode.STORE_ERROR: OuisyncError_StoreError,
    ErrorCode.IS_DIRECTORY: OuisyncError_IsDirectory,
    ErrorCode.NOT_DIRECTORY: OuisyncError_NotDirectory,
    ErrorCode.DIRECTORY_NOT_EMPTY: OuisyncError_DirectoryNotEmpty,
    ErrorCode.RESOURCE_BUSY: OuisyncError_ResourceBusy,
    ErrorCode.RUNTIME_INITIALIZE_ERROR: OuisyncError_RuntimeInitializeError,
    ErrorCode.CONFIG_ERROR: OuisyncError_ConfigError,
    ErrorCode.TLS_CERTIFICATES_NOT_FOUND: OuisyncError_TlsCertificatesNotFound,
    ErrorCode.TLS_CERTIFICATES_INVALID: OuisyncError_TlsCertificatesInvalid,
    ErrorCode.TLS_KEYS_NOT_FOUND: OuisyncError_TlsKeysNotFound,
    ErrorCode.TLS_CONFIG_ERROR: OuisyncError_TlsConfigError,
    ErrorCode.VFS_DRIVER_INSTALL_ERROR: OuisyncError_VfsDriverInstallError,
    ErrorCode.VFS_OTHER_ERROR: OuisyncError_VfsOtherError,
    ErrorCode.SERVICE_ALREADY_RUNNING: OuisyncError_ServiceAlreadyRunning,
    ErrorCode.STORE_DIR_UNSPECIFIED: OuisyncError_StoreDirUnspecified,
    ErrorCode.MOUNT_DIR_UNSPECIFIED: OuisyncError_MountDirUnspecified,
}

class LogLevel(IntEnum):
    ERROR = 1
    WARN = 2
    INFO = 3
    DEBUG = 4
    TRACE = 5

@dataclass
class MessageId:
    _shape: ClassVar[str] = "unnamed"
    value: int

@dataclass
class MetadataEdit:
    _shape: ClassVar[str] = "named"
    key: str
    old_value: str | None
    new_value: str | None

@dataclass
class NetworkDefaults:
    _shape: ClassVar[str] = "named"
    bind: list[str]
    port_forwarding_enabled: bool
    local_discovery_enabled: bool

@dataclass
class DirectoryEntry:
    _shape: ClassVar[str] = "named"
    name: str
    entry_type: EntryType

@dataclass
class QuotaInfo:
    _shape: ClassVar[str] = "named"
    quota: StorageSize | None
    size: StorageSize

@dataclass
class Datagram:
    _shape: ClassVar[str] = "named"
    data: bytes
    addr: str

@dataclass
class FileHandle:
    _shape: ClassVar[str] = "unnamed"
    value: int

@dataclass
class RepositoryHandle:
    _shape: ClassVar[str] = "unnamed"
    value: int

@dataclass
class NetworkSocketHandle:
    _shape: ClassVar[str] = "unnamed"
    value: int

@dataclass
class NetworkStreamHandle:
    _shape: ClassVar[str] = "unnamed"
    value: int

class Request:
    _variants: ClassVar[dict[str, type]] = {}

@dataclass
class Request_Cancel(Request):
    _tag: ClassVar[str] = "Cancel"
    _shape: ClassVar[str] = "named"
    id: MessageId

@dataclass
class Request_FileClose(Request):
    _tag: ClassVar[str] = "FileClose"
    _shape: ClassVar[str] = "named"
    file: FileHandle

@dataclass
class Request_FileFlush(Request):
    _tag: ClassVar[str] = "FileFlush"
    _shape: ClassVar[str] = "named"
    file: FileHandle

@dataclass
class Request_FileGetLength(Request):
    _tag: ClassVar[str] = "FileGetLength"
    _shape: ClassVar[str] = "named"
    file: FileHandle

@dataclass
class Request_FileGetProgress(Request):
    _tag: ClassVar[str] = "FileGetProgress"
    _shape: ClassVar[str] = "named"
    file: FileHandle

@dataclass
class Request_FileRead(Request):
    _tag: ClassVar[str] = "FileRead"
    _shape: ClassVar[str] = "named"
    file: FileHandle
    offset: int
    size: int

@dataclass
class Request_FileTruncate(Request):
    _tag: ClassVar[str] = "FileTruncate"
    _shape: ClassVar[str] = "named"
    file: FileHandle
    len: int

@dataclass
class Request_FileWrite(Request):
    _tag: ClassVar[str] = "FileWrite"
    _shape: ClassVar[str] = "named"
    file: FileHandle
    offset: int
    data: bytes

@dataclass
class Request_NetworkSocketClose(Request):
    _tag: ClassVar[str] = "NetworkSocketClose"
    _shape: ClassVar[str] = "named"
    socket: NetworkSocketHandle

@dataclass
class Request_NetworkSocketRecvFrom(Request):
    _tag: ClassVar[str] = "NetworkSocketRecvFrom"
    _shape: ClassVar[str] = "named"
    socket: NetworkSocketHandle
    len: int

@dataclass
class Request_NetworkSocketSendTo(Request):
    _tag: ClassVar[str] = "NetworkSocketSendTo"
    _shape: ClassVar[str] = "named"
    socket: NetworkSocketHandle
    data: bytes
    addr: str

@dataclass
class Request_NetworkStreamClose(Request):
    _tag: ClassVar[str] = "NetworkStreamClose"
    _shape: ClassVar[str] = "named"
    stream: NetworkStreamHandle

@dataclass
class Request_NetworkStreamReadExact(Request):
    _tag: ClassVar[str] = "NetworkStreamReadExact"
    _shape: ClassVar[str] = "named"
    stream: NetworkStreamHandle
    len: int

@dataclass
class Request_NetworkStreamWriteAll(Request):
    _tag: ClassVar[str] = "NetworkStreamWriteAll"
    _shape: ClassVar[str] = "named"
    stream: NetworkStreamHandle
    buf: bytes

@dataclass
class Request_RepositoryClose(Request):
    _tag: ClassVar[str] = "RepositoryClose"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryCreateDirectory(Request):
    _tag: ClassVar[str] = "RepositoryCreateDirectory"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    path: str

@dataclass
class Request_RepositoryCreateFile(Request):
    _tag: ClassVar[str] = "RepositoryCreateFile"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    path: str

@dataclass
class Request_RepositoryCreateMirror(Request):
    _tag: ClassVar[str] = "RepositoryCreateMirror"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    host: str

@dataclass
class Request_RepositoryDelete(Request):
    _tag: ClassVar[str] = "RepositoryDelete"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryDeleteMirror(Request):
    _tag: ClassVar[str] = "RepositoryDeleteMirror"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    host: str

@dataclass
class Request_RepositoryExport(Request):
    _tag: ClassVar[str] = "RepositoryExport"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    output_path: str

@dataclass
class Request_RepositoryFileExists(Request):
    _tag: ClassVar[str] = "RepositoryFileExists"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    path: str

@dataclass
class Request_RepositoryGetAccessMode(Request):
    _tag: ClassVar[str] = "RepositoryGetAccessMode"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryGetBlockExpiration(Request):
    _tag: ClassVar[str] = "RepositoryGetBlockExpiration"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryGetCredentials(Request):
    _tag: ClassVar[str] = "RepositoryGetCredentials"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryGetEntryType(Request):
    _tag: ClassVar[str] = "RepositoryGetEntryType"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    path: str

@dataclass
class Request_RepositoryGetExpiration(Request):
    _tag: ClassVar[str] = "RepositoryGetExpiration"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryGetInfoHash(Request):
    _tag: ClassVar[str] = "RepositoryGetInfoHash"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryGetMetadata(Request):
    _tag: ClassVar[str] = "RepositoryGetMetadata"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    key: str

@dataclass
class Request_RepositoryGetMountPoint(Request):
    _tag: ClassVar[str] = "RepositoryGetMountPoint"
    _shape: ClassVar[str] = "named"
    _repo: RepositoryHandle

@dataclass
class Request_RepositoryGetPath(Request):
    _tag: ClassVar[str] = "RepositoryGetPath"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryGetQuota(Request):
    _tag: ClassVar[str] = "RepositoryGetQuota"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryGetShortName(Request):
    _tag: ClassVar[str] = "RepositoryGetShortName"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryGetStats(Request):
    _tag: ClassVar[str] = "RepositoryGetStats"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryGetSyncProgress(Request):
    _tag: ClassVar[str] = "RepositoryGetSyncProgress"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryIsDhtEnabled(Request):
    _tag: ClassVar[str] = "RepositoryIsDhtEnabled"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryIsPexEnabled(Request):
    _tag: ClassVar[str] = "RepositoryIsPexEnabled"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryIsSyncEnabled(Request):
    _tag: ClassVar[str] = "RepositoryIsSyncEnabled"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryMirrorExists(Request):
    _tag: ClassVar[str] = "RepositoryMirrorExists"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    host: str

@dataclass
class Request_RepositoryMount(Request):
    _tag: ClassVar[str] = "RepositoryMount"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryMove(Request):
    _tag: ClassVar[str] = "RepositoryMove"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    dst: str

@dataclass
class Request_RepositoryMoveEntry(Request):
    _tag: ClassVar[str] = "RepositoryMoveEntry"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    src: str
    dst: str

@dataclass
class Request_RepositoryOpenFile(Request):
    _tag: ClassVar[str] = "RepositoryOpenFile"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    path: str

@dataclass
class Request_RepositoryReadDirectory(Request):
    _tag: ClassVar[str] = "RepositoryReadDirectory"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    path: str

@dataclass
class Request_RepositoryRemoveDirectory(Request):
    _tag: ClassVar[str] = "RepositoryRemoveDirectory"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    path: str
    recursive: bool

@dataclass
class Request_RepositoryRemoveFile(Request):
    _tag: ClassVar[str] = "RepositoryRemoveFile"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    path: str

@dataclass
class Request_RepositoryResetAccess(Request):
    _tag: ClassVar[str] = "RepositoryResetAccess"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    token: str

@dataclass
class Request_RepositorySetAccess(Request):
    _tag: ClassVar[str] = "RepositorySetAccess"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    read: AccessChange | None
    write: AccessChange | None

@dataclass
class Request_RepositorySetAccessMode(Request):
    _tag: ClassVar[str] = "RepositorySetAccessMode"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    access_mode: AccessMode
    local_secret: LocalSecret | None

@dataclass
class Request_RepositorySetBlockExpiration(Request):
    _tag: ClassVar[str] = "RepositorySetBlockExpiration"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    value: int | None

@dataclass
class Request_RepositorySetCredentials(Request):
    _tag: ClassVar[str] = "RepositorySetCredentials"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    credentials: bytes

@dataclass
class Request_RepositorySetDhtEnabled(Request):
    _tag: ClassVar[str] = "RepositorySetDhtEnabled"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    enabled: bool

@dataclass
class Request_RepositorySetExpiration(Request):
    _tag: ClassVar[str] = "RepositorySetExpiration"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    value: int | None

@dataclass
class Request_RepositorySetMetadata(Request):
    _tag: ClassVar[str] = "RepositorySetMetadata"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    edits: list[MetadataEdit]

@dataclass
class Request_RepositorySetPexEnabled(Request):
    _tag: ClassVar[str] = "RepositorySetPexEnabled"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    enabled: bool

@dataclass
class Request_RepositorySetQuota(Request):
    _tag: ClassVar[str] = "RepositorySetQuota"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    value: StorageSize | None

@dataclass
class Request_RepositorySetSyncEnabled(Request):
    _tag: ClassVar[str] = "RepositorySetSyncEnabled"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    enabled: bool

@dataclass
class Request_RepositoryShare(Request):
    _tag: ClassVar[str] = "RepositoryShare"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle
    access_mode: AccessMode
    local_secret: LocalSecret | None

@dataclass
class Request_RepositorySubscribe(Request):
    _tag: ClassVar[str] = "RepositorySubscribe"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_RepositoryUnmount(Request):
    _tag: ClassVar[str] = "RepositoryUnmount"
    _shape: ClassVar[str] = "named"
    repo: RepositoryHandle

@dataclass
class Request_SessionAddUserProvidedPeers(Request):
    _tag: ClassVar[str] = "SessionAddUserProvidedPeers"
    _shape: ClassVar[str] = "named"
    addrs: list[str]

@dataclass
class Request_SessionBindMetrics(Request):
    _tag: ClassVar[str] = "SessionBindMetrics"
    _shape: ClassVar[str] = "named"
    addr: str | None

@dataclass
class Request_SessionBindNetwork(Request):
    _tag: ClassVar[str] = "SessionBindNetwork"
    _shape: ClassVar[str] = "named"
    addrs: list[str]

@dataclass
class Request_SessionBindRemoteControl(Request):
    _tag: ClassVar[str] = "SessionBindRemoteControl"
    _shape: ClassVar[str] = "named"
    addr: str | None

@dataclass
class Request_SessionCopy(Request):
    _tag: ClassVar[str] = "SessionCopy"
    _shape: ClassVar[str] = "named"
    src_repo: str | None
    src_path: str
    dst_repo: str | None
    dst_path: str

@dataclass
class Request_SessionCreateRepository(Request):
    _tag: ClassVar[str] = "SessionCreateRepository"
    _shape: ClassVar[str] = "named"
    path: str
    read_secret: SetLocalSecret | None
    write_secret: SetLocalSecret | None
    token: str | None
    sync_enabled: bool
    dht_enabled: bool
    pex_enabled: bool

@dataclass
class Request_SessionDeleteRepositoryByName(Request):
    _tag: ClassVar[str] = "SessionDeleteRepositoryByName"
    _shape: ClassVar[str] = "named"
    name: str

@dataclass
class Request_SessionDeriveSecretKey(Request):
    _tag: ClassVar[str] = "SessionDeriveSecretKey"
    _shape: ClassVar[str] = "named"
    password: Password
    salt: PasswordSalt

@dataclass
class Request_SessionDhtLookup(Request):
    _tag: ClassVar[str] = "SessionDhtLookup"
    _shape: ClassVar[str] = "named"
    info_hash: str
    announce: bool

@dataclass
class Request_SessionFindRepository(Request):
    _tag: ClassVar[str] = "SessionFindRepository"
    _shape: ClassVar[str] = "named"
    name: str

@dataclass
class Request_SessionGeneratePasswordSalt(Request):
    _tag: ClassVar[str] = "SessionGeneratePasswordSalt"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGenerateSecretKey(Request):
    _tag: ClassVar[str] = "SessionGenerateSecretKey"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetCurrentProtocolVersion(Request):
    _tag: ClassVar[str] = "SessionGetCurrentProtocolVersion"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetDefaultBlockExpiration(Request):
    _tag: ClassVar[str] = "SessionGetDefaultBlockExpiration"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetDefaultQuota(Request):
    _tag: ClassVar[str] = "SessionGetDefaultQuota"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetDefaultRepositoryExpiration(Request):
    _tag: ClassVar[str] = "SessionGetDefaultRepositoryExpiration"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetDhtRouters(Request):
    _tag: ClassVar[str] = "SessionGetDhtRouters"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetExternalAddrV4(Request):
    _tag: ClassVar[str] = "SessionGetExternalAddrV4"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetExternalAddrV6(Request):
    _tag: ClassVar[str] = "SessionGetExternalAddrV6"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetHighestSeenProtocolVersion(Request):
    _tag: ClassVar[str] = "SessionGetHighestSeenProtocolVersion"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetLocalListenerAddrs(Request):
    _tag: ClassVar[str] = "SessionGetLocalListenerAddrs"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetMetricsListenerAddr(Request):
    _tag: ClassVar[str] = "SessionGetMetricsListenerAddr"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetMountRoot(Request):
    _tag: ClassVar[str] = "SessionGetMountRoot"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetNatBehavior(Request):
    _tag: ClassVar[str] = "SessionGetNatBehavior"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetNetworkStats(Request):
    _tag: ClassVar[str] = "SessionGetNetworkStats"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetPeers(Request):
    _tag: ClassVar[str] = "SessionGetPeers"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetRemoteControlListenerAddr(Request):
    _tag: ClassVar[str] = "SessionGetRemoteControlListenerAddr"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetRemoteListenerAddrs(Request):
    _tag: ClassVar[str] = "SessionGetRemoteListenerAddrs"
    _shape: ClassVar[str] = "named"
    host: str

@dataclass
class Request_SessionGetRuntimeId(Request):
    _tag: ClassVar[str] = "SessionGetRuntimeId"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetShareTokenAccessMode(Request):
    _tag: ClassVar[str] = "SessionGetShareTokenAccessMode"
    _shape: ClassVar[str] = "named"
    token: str

@dataclass
class Request_SessionGetShareTokenInfoHash(Request):
    _tag: ClassVar[str] = "SessionGetShareTokenInfoHash"
    _shape: ClassVar[str] = "named"
    token: str

@dataclass
class Request_SessionGetShareTokenSuggestedName(Request):
    _tag: ClassVar[str] = "SessionGetShareTokenSuggestedName"
    _shape: ClassVar[str] = "named"
    token: str

@dataclass
class Request_SessionGetStateMonitor(Request):
    _tag: ClassVar[str] = "SessionGetStateMonitor"
    _shape: ClassVar[str] = "named"
    path: list[MonitorId]

@dataclass
class Request_SessionGetStoreDirs(Request):
    _tag: ClassVar[str] = "SessionGetStoreDirs"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionGetUserProvidedPeers(Request):
    _tag: ClassVar[str] = "SessionGetUserProvidedPeers"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionInitNetwork(Request):
    _tag: ClassVar[str] = "SessionInitNetwork"
    _shape: ClassVar[str] = "named"
    defaults: NetworkDefaults

@dataclass
class Request_SessionInsertStoreDirs(Request):
    _tag: ClassVar[str] = "SessionInsertStoreDirs"
    _shape: ClassVar[str] = "named"
    paths: list[str]

@dataclass
class Request_SessionIsLocalDhtEnabled(Request):
    _tag: ClassVar[str] = "SessionIsLocalDhtEnabled"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionIsLocalDiscoveryEnabled(Request):
    _tag: ClassVar[str] = "SessionIsLocalDiscoveryEnabled"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionIsPexRecvEnabled(Request):
    _tag: ClassVar[str] = "SessionIsPexRecvEnabled"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionIsPexSendEnabled(Request):
    _tag: ClassVar[str] = "SessionIsPexSendEnabled"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionIsPortForwardingEnabled(Request):
    _tag: ClassVar[str] = "SessionIsPortForwardingEnabled"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionListRepositories(Request):
    _tag: ClassVar[str] = "SessionListRepositories"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionMirrorExists(Request):
    _tag: ClassVar[str] = "SessionMirrorExists"
    _shape: ClassVar[str] = "named"
    token: str
    host: str

@dataclass
class Request_SessionOpenNetworkSocketV4(Request):
    _tag: ClassVar[str] = "SessionOpenNetworkSocketV4"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionOpenNetworkSocketV6(Request):
    _tag: ClassVar[str] = "SessionOpenNetworkSocketV6"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionOpenNetworkStream(Request):
    _tag: ClassVar[str] = "SessionOpenNetworkStream"
    _shape: ClassVar[str] = "named"
    addr: str
    topic_id: TopicId

@dataclass
class Request_SessionOpenRepository(Request):
    _tag: ClassVar[str] = "SessionOpenRepository"
    _shape: ClassVar[str] = "named"
    path: str
    local_secret: LocalSecret | None

@dataclass
class Request_SessionPinDht(Request):
    _tag: ClassVar[str] = "SessionPinDht"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionRemoveStoreDirs(Request):
    _tag: ClassVar[str] = "SessionRemoveStoreDirs"
    _shape: ClassVar[str] = "named"
    paths: list[str]

@dataclass
class Request_SessionRemoveUserProvidedPeers(Request):
    _tag: ClassVar[str] = "SessionRemoveUserProvidedPeers"
    _shape: ClassVar[str] = "named"
    addrs: list[str]

@dataclass
class Request_SessionSetDefaultBlockExpiration(Request):
    _tag: ClassVar[str] = "SessionSetDefaultBlockExpiration"
    _shape: ClassVar[str] = "named"
    value: int | None

@dataclass
class Request_SessionSetDefaultQuota(Request):
    _tag: ClassVar[str] = "SessionSetDefaultQuota"
    _shape: ClassVar[str] = "named"
    value: StorageSize | None

@dataclass
class Request_SessionSetDefaultRepositoryExpiration(Request):
    _tag: ClassVar[str] = "SessionSetDefaultRepositoryExpiration"
    _shape: ClassVar[str] = "named"
    value: int | None

@dataclass
class Request_SessionSetDhtRouters(Request):
    _tag: ClassVar[str] = "SessionSetDhtRouters"
    _shape: ClassVar[str] = "named"
    routers: list[str]

@dataclass
class Request_SessionSetLocalDhtEnabled(Request):
    _tag: ClassVar[str] = "SessionSetLocalDhtEnabled"
    _shape: ClassVar[str] = "named"
    enabled: bool

@dataclass
class Request_SessionSetLocalDiscoveryEnabled(Request):
    _tag: ClassVar[str] = "SessionSetLocalDiscoveryEnabled"
    _shape: ClassVar[str] = "named"
    enabled: bool

@dataclass
class Request_SessionSetMountRoot(Request):
    _tag: ClassVar[str] = "SessionSetMountRoot"
    _shape: ClassVar[str] = "named"
    path: str | None

@dataclass
class Request_SessionSetPexRecvEnabled(Request):
    _tag: ClassVar[str] = "SessionSetPexRecvEnabled"
    _shape: ClassVar[str] = "named"
    enabled: bool

@dataclass
class Request_SessionSetPexSendEnabled(Request):
    _tag: ClassVar[str] = "SessionSetPexSendEnabled"
    _shape: ClassVar[str] = "named"
    enabled: bool

@dataclass
class Request_SessionSetPortForwardingEnabled(Request):
    _tag: ClassVar[str] = "SessionSetPortForwardingEnabled"
    _shape: ClassVar[str] = "named"
    enabled: bool

@dataclass
class Request_SessionSetStoreDirs(Request):
    _tag: ClassVar[str] = "SessionSetStoreDirs"
    _shape: ClassVar[str] = "named"
    paths: list[str]

@dataclass
class Request_SessionSubscribeToNetwork(Request):
    _tag: ClassVar[str] = "SessionSubscribeToNetwork"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionSubscribeToStateMonitor(Request):
    _tag: ClassVar[str] = "SessionSubscribeToStateMonitor"
    _shape: ClassVar[str] = "named"
    path: list[MonitorId]

@dataclass
class Request_SessionUnpinDht(Request):
    _tag: ClassVar[str] = "SessionUnpinDht"
    _shape: ClassVar[str] = "unit"

@dataclass
class Request_SessionValidateShareToken(Request):
    _tag: ClassVar[str] = "SessionValidateShareToken"
    _shape: ClassVar[str] = "named"
    token: str

Request._variants = {
    "Cancel": Request_Cancel,
    "FileClose": Request_FileClose,
    "FileFlush": Request_FileFlush,
    "FileGetLength": Request_FileGetLength,
    "FileGetProgress": Request_FileGetProgress,
    "FileRead": Request_FileRead,
    "FileTruncate": Request_FileTruncate,
    "FileWrite": Request_FileWrite,
    "NetworkSocketClose": Request_NetworkSocketClose,
    "NetworkSocketRecvFrom": Request_NetworkSocketRecvFrom,
    "NetworkSocketSendTo": Request_NetworkSocketSendTo,
    "NetworkStreamClose": Request_NetworkStreamClose,
    "NetworkStreamReadExact": Request_NetworkStreamReadExact,
    "NetworkStreamWriteAll": Request_NetworkStreamWriteAll,
    "RepositoryClose": Request_RepositoryClose,
    "RepositoryCreateDirectory": Request_RepositoryCreateDirectory,
    "RepositoryCreateFile": Request_RepositoryCreateFile,
    "RepositoryCreateMirror": Request_RepositoryCreateMirror,
    "RepositoryDelete": Request_RepositoryDelete,
    "RepositoryDeleteMirror": Request_RepositoryDeleteMirror,
    "RepositoryExport": Request_RepositoryExport,
    "RepositoryFileExists": Request_RepositoryFileExists,
    "RepositoryGetAccessMode": Request_RepositoryGetAccessMode,
    "RepositoryGetBlockExpiration": Request_RepositoryGetBlockExpiration,
    "RepositoryGetCredentials": Request_RepositoryGetCredentials,
    "RepositoryGetEntryType": Request_RepositoryGetEntryType,
    "RepositoryGetExpiration": Request_RepositoryGetExpiration,
    "RepositoryGetInfoHash": Request_RepositoryGetInfoHash,
    "RepositoryGetMetadata": Request_RepositoryGetMetadata,
    "RepositoryGetMountPoint": Request_RepositoryGetMountPoint,
    "RepositoryGetPath": Request_RepositoryGetPath,
    "RepositoryGetQuota": Request_RepositoryGetQuota,
    "RepositoryGetShortName": Request_RepositoryGetShortName,
    "RepositoryGetStats": Request_RepositoryGetStats,
    "RepositoryGetSyncProgress": Request_RepositoryGetSyncProgress,
    "RepositoryIsDhtEnabled": Request_RepositoryIsDhtEnabled,
    "RepositoryIsPexEnabled": Request_RepositoryIsPexEnabled,
    "RepositoryIsSyncEnabled": Request_RepositoryIsSyncEnabled,
    "RepositoryMirrorExists": Request_RepositoryMirrorExists,
    "RepositoryMount": Request_RepositoryMount,
    "RepositoryMove": Request_RepositoryMove,
    "RepositoryMoveEntry": Request_RepositoryMoveEntry,
    "RepositoryOpenFile": Request_RepositoryOpenFile,
    "RepositoryReadDirectory": Request_RepositoryReadDirectory,
    "RepositoryRemoveDirectory": Request_RepositoryRemoveDirectory,
    "RepositoryRemoveFile": Request_RepositoryRemoveFile,
    "RepositoryResetAccess": Request_RepositoryResetAccess,
    "RepositorySetAccess": Request_RepositorySetAccess,
    "RepositorySetAccessMode": Request_RepositorySetAccessMode,
    "RepositorySetBlockExpiration": Request_RepositorySetBlockExpiration,
    "RepositorySetCredentials": Request_RepositorySetCredentials,
    "RepositorySetDhtEnabled": Request_RepositorySetDhtEnabled,
    "RepositorySetExpiration": Request_RepositorySetExpiration,
    "RepositorySetMetadata": Request_RepositorySetMetadata,
    "RepositorySetPexEnabled": Request_RepositorySetPexEnabled,
    "RepositorySetQuota": Request_RepositorySetQuota,
    "RepositorySetSyncEnabled": Request_RepositorySetSyncEnabled,
    "RepositoryShare": Request_RepositoryShare,
    "RepositorySubscribe": Request_RepositorySubscribe,
    "RepositoryUnmount": Request_RepositoryUnmount,
    "SessionAddUserProvidedPeers": Request_SessionAddUserProvidedPeers,
    "SessionBindMetrics": Request_SessionBindMetrics,
    "SessionBindNetwork": Request_SessionBindNetwork,
    "SessionBindRemoteControl": Request_SessionBindRemoteControl,
    "SessionCopy": Request_SessionCopy,
    "SessionCreateRepository": Request_SessionCreateRepository,
    "SessionDeleteRepositoryByName": Request_SessionDeleteRepositoryByName,
    "SessionDeriveSecretKey": Request_SessionDeriveSecretKey,
    "SessionDhtLookup": Request_SessionDhtLookup,
    "SessionFindRepository": Request_SessionFindRepository,
    "SessionGeneratePasswordSalt": Request_SessionGeneratePasswordSalt,
    "SessionGenerateSecretKey": Request_SessionGenerateSecretKey,
    "SessionGetCurrentProtocolVersion": Request_SessionGetCurrentProtocolVersion,
    "SessionGetDefaultBlockExpiration": Request_SessionGetDefaultBlockExpiration,
    "SessionGetDefaultQuota": Request_SessionGetDefaultQuota,
    "SessionGetDefaultRepositoryExpiration": Request_SessionGetDefaultRepositoryExpiration,
    "SessionGetDhtRouters": Request_SessionGetDhtRouters,
    "SessionGetExternalAddrV4": Request_SessionGetExternalAddrV4,
    "SessionGetExternalAddrV6": Request_SessionGetExternalAddrV6,
    "SessionGetHighestSeenProtocolVersion": Request_SessionGetHighestSeenProtocolVersion,
    "SessionGetLocalListenerAddrs": Request_SessionGetLocalListenerAddrs,
    "SessionGetMetricsListenerAddr": Request_SessionGetMetricsListenerAddr,
    "SessionGetMountRoot": Request_SessionGetMountRoot,
    "SessionGetNatBehavior": Request_SessionGetNatBehavior,
    "SessionGetNetworkStats": Request_SessionGetNetworkStats,
    "SessionGetPeers": Request_SessionGetPeers,
    "SessionGetRemoteControlListenerAddr": Request_SessionGetRemoteControlListenerAddr,
    "SessionGetRemoteListenerAddrs": Request_SessionGetRemoteListenerAddrs,
    "SessionGetRuntimeId": Request_SessionGetRuntimeId,
    "SessionGetShareTokenAccessMode": Request_SessionGetShareTokenAccessMode,
    "SessionGetShareTokenInfoHash": Request_SessionGetShareTokenInfoHash,
    "SessionGetShareTokenSuggestedName": Request_SessionGetShareTokenSuggestedName,
    "SessionGetStateMonitor": Request_SessionGetStateMonitor,
    "SessionGetStoreDirs": Request_SessionGetStoreDirs,
    "SessionGetUserProvidedPeers": Request_SessionGetUserProvidedPeers,
    "SessionInitNetwork": Request_SessionInitNetwork,
    "SessionInsertStoreDirs": Request_SessionInsertStoreDirs,
    "SessionIsLocalDhtEnabled": Request_SessionIsLocalDhtEnabled,
    "SessionIsLocalDiscoveryEnabled": Request_SessionIsLocalDiscoveryEnabled,
    "SessionIsPexRecvEnabled": Request_SessionIsPexRecvEnabled,
    "SessionIsPexSendEnabled": Request_SessionIsPexSendEnabled,
    "SessionIsPortForwardingEnabled": Request_SessionIsPortForwardingEnabled,
    "SessionListRepositories": Request_SessionListRepositories,
    "SessionMirrorExists": Request_SessionMirrorExists,
    "SessionOpenNetworkSocketV4": Request_SessionOpenNetworkSocketV4,
    "SessionOpenNetworkSocketV6": Request_SessionOpenNetworkSocketV6,
    "SessionOpenNetworkStream": Request_SessionOpenNetworkStream,
    "SessionOpenRepository": Request_SessionOpenRepository,
    "SessionPinDht": Request_SessionPinDht,
    "SessionRemoveStoreDirs": Request_SessionRemoveStoreDirs,
    "SessionRemoveUserProvidedPeers": Request_SessionRemoveUserProvidedPeers,
    "SessionSetDefaultBlockExpiration": Request_SessionSetDefaultBlockExpiration,
    "SessionSetDefaultQuota": Request_SessionSetDefaultQuota,
    "SessionSetDefaultRepositoryExpiration": Request_SessionSetDefaultRepositoryExpiration,
    "SessionSetDhtRouters": Request_SessionSetDhtRouters,
    "SessionSetLocalDhtEnabled": Request_SessionSetLocalDhtEnabled,
    "SessionSetLocalDiscoveryEnabled": Request_SessionSetLocalDiscoveryEnabled,
    "SessionSetMountRoot": Request_SessionSetMountRoot,
    "SessionSetPexRecvEnabled": Request_SessionSetPexRecvEnabled,
    "SessionSetPexSendEnabled": Request_SessionSetPexSendEnabled,
    "SessionSetPortForwardingEnabled": Request_SessionSetPortForwardingEnabled,
    "SessionSetStoreDirs": Request_SessionSetStoreDirs,
    "SessionSubscribeToNetwork": Request_SessionSubscribeToNetwork,
    "SessionSubscribeToStateMonitor": Request_SessionSubscribeToStateMonitor,
    "SessionUnpinDht": Request_SessionUnpinDht,
    "SessionValidateShareToken": Request_SessionValidateShareToken,
}

class Response:
    _variants: ClassVar[dict[str, type]] = {}

@dataclass
class Response_AccessMode(Response):
    _tag: ClassVar[str] = "AccessMode"
    _shape: ClassVar[str] = "unnamed"
    value: AccessMode

@dataclass
class Response_Bool(Response):
    _tag: ClassVar[str] = "Bool"
    _shape: ClassVar[str] = "unnamed"
    value: bool

@dataclass
class Response_Bytes(Response):
    _tag: ClassVar[str] = "Bytes"
    _shape: ClassVar[str] = "unnamed"
    value: bytes

@dataclass
class Response_Datagram(Response):
    _tag: ClassVar[str] = "Datagram"
    _shape: ClassVar[str] = "unnamed"
    value: Datagram

@dataclass
class Response_DirectoryEntries(Response):
    _tag: ClassVar[str] = "DirectoryEntries"
    _shape: ClassVar[str] = "unnamed"
    value: list[DirectoryEntry]

@dataclass
class Response_Duration(Response):
    _tag: ClassVar[str] = "Duration"
    _shape: ClassVar[str] = "unnamed"
    value: int

@dataclass
class Response_EntryType(Response):
    _tag: ClassVar[str] = "EntryType"
    _shape: ClassVar[str] = "unnamed"
    value: EntryType

@dataclass
class Response_File(Response):
    _tag: ClassVar[str] = "File"
    _shape: ClassVar[str] = "unnamed"
    value: FileHandle

@dataclass
class Response_NatBehavior(Response):
    _tag: ClassVar[str] = "NatBehavior"
    _shape: ClassVar[str] = "unnamed"
    value: NatBehavior

@dataclass
class Response_NetworkEvent(Response):
    _tag: ClassVar[str] = "NetworkEvent"
    _shape: ClassVar[str] = "unnamed"
    value: NetworkEvent

@dataclass
class Response_NetworkSocket(Response):
    _tag: ClassVar[str] = "NetworkSocket"
    _shape: ClassVar[str] = "unnamed"
    value: NetworkSocketHandle

@dataclass
class Response_NetworkStream(Response):
    _tag: ClassVar[str] = "NetworkStream"
    _shape: ClassVar[str] = "unnamed"
    value: NetworkStreamHandle

@dataclass
class Response_None(Response):
    _tag: ClassVar[str] = "None"
    _shape: ClassVar[str] = "unit"

@dataclass
class Response_PasswordSalt(Response):
    _tag: ClassVar[str] = "PasswordSalt"
    _shape: ClassVar[str] = "unnamed"
    value: PasswordSalt

@dataclass
class Response_Path(Response):
    _tag: ClassVar[str] = "Path"
    _shape: ClassVar[str] = "unnamed"
    value: str

@dataclass
class Response_Paths(Response):
    _tag: ClassVar[str] = "Paths"
    _shape: ClassVar[str] = "unnamed"
    value: list[str]

@dataclass
class Response_PeerAddr(Response):
    _tag: ClassVar[str] = "PeerAddr"
    _shape: ClassVar[str] = "unnamed"
    value: str

@dataclass
class Response_PeerAddrs(Response):
    _tag: ClassVar[str] = "PeerAddrs"
    _shape: ClassVar[str] = "unnamed"
    value: list[str]

@dataclass
class Response_PeerInfos(Response):
    _tag: ClassVar[str] = "PeerInfos"
    _shape: ClassVar[str] = "unnamed"
    value: list[PeerInfo]

@dataclass
class Response_Progress(Response):
    _tag: ClassVar[str] = "Progress"
    _shape: ClassVar[str] = "unnamed"
    value: Progress

@dataclass
class Response_PublicRuntimeId(Response):
    _tag: ClassVar[str] = "PublicRuntimeId"
    _shape: ClassVar[str] = "unnamed"
    value: PublicRuntimeId

@dataclass
class Response_QuotaInfo(Response):
    _tag: ClassVar[str] = "QuotaInfo"
    _shape: ClassVar[str] = "unnamed"
    value: QuotaInfo

@dataclass
class Response_Repositories(Response):
    _tag: ClassVar[str] = "Repositories"
    _shape: ClassVar[str] = "unnamed"
    value: dict[str, RepositoryHandle]

@dataclass
class Response_Repository(Response):
    _tag: ClassVar[str] = "Repository"
    _shape: ClassVar[str] = "unnamed"
    value: RepositoryHandle

@dataclass
class Response_SecretKey(Response):
    _tag: ClassVar[str] = "SecretKey"
    _shape: ClassVar[str] = "unnamed"
    value: SecretKey

@dataclass
class Response_ShareToken(Response):
    _tag: ClassVar[str] = "ShareToken"
    _shape: ClassVar[str] = "unnamed"
    value: str

@dataclass
class Response_SocketAddr(Response):
    _tag: ClassVar[str] = "SocketAddr"
    _shape: ClassVar[str] = "unnamed"
    value: str

@dataclass
class Response_StateMonitor(Response):
    _tag: ClassVar[str] = "StateMonitor"
    _shape: ClassVar[str] = "unnamed"
    value: typing.Any

@dataclass
class Response_Stats(Response):
    _tag: ClassVar[str] = "Stats"
    _shape: ClassVar[str] = "unnamed"
    value: Stats

@dataclass
class Response_StorageSize(Response):
    _tag: ClassVar[str] = "StorageSize"
    _shape: ClassVar[str] = "unnamed"
    value: StorageSize

@dataclass
class Response_String(Response):
    _tag: ClassVar[str] = "String"
    _shape: ClassVar[str] = "unnamed"
    value: str

@dataclass
class Response_Strings(Response):
    _tag: ClassVar[str] = "Strings"
    _shape: ClassVar[str] = "unnamed"
    value: list[str]

@dataclass
class Response_U16(Response):
    _tag: ClassVar[str] = "U16"
    _shape: ClassVar[str] = "unnamed"
    value: int

@dataclass
class Response_U64(Response):
    _tag: ClassVar[str] = "U64"
    _shape: ClassVar[str] = "unnamed"
    value: int

@dataclass
class Response_Unit(Response):
    _tag: ClassVar[str] = "Unit"
    _shape: ClassVar[str] = "unit"

Response._variants = {
    "AccessMode": Response_AccessMode,
    "Bool": Response_Bool,
    "Bytes": Response_Bytes,
    "Datagram": Response_Datagram,
    "DirectoryEntries": Response_DirectoryEntries,
    "Duration": Response_Duration,
    "EntryType": Response_EntryType,
    "File": Response_File,
    "NatBehavior": Response_NatBehavior,
    "NetworkEvent": Response_NetworkEvent,
    "NetworkSocket": Response_NetworkSocket,
    "NetworkStream": Response_NetworkStream,
    "None": Response_None,
    "PasswordSalt": Response_PasswordSalt,
    "Path": Response_Path,
    "Paths": Response_Paths,
    "PeerAddr": Response_PeerAddr,
    "PeerAddrs": Response_PeerAddrs,
    "PeerInfos": Response_PeerInfos,
    "Progress": Response_Progress,
    "PublicRuntimeId": Response_PublicRuntimeId,
    "QuotaInfo": Response_QuotaInfo,
    "Repositories": Response_Repositories,
    "Repository": Response_Repository,
    "SecretKey": Response_SecretKey,
    "ShareToken": Response_ShareToken,
    "SocketAddr": Response_SocketAddr,
    "StateMonitor": Response_StateMonitor,
    "Stats": Response_Stats,
    "StorageSize": Response_StorageSize,
    "String": Response_String,
    "Strings": Response_Strings,
    "U16": Response_U16,
    "U64": Response_U64,
    "Unit": Response_Unit,
}

class Session:
    def __init__(self, client: "Client"):
        self._client = client

    async def add_user_provided_peers(
        self,
        *,
        addrs: list[str],
    ):
        request = Request_SessionAddUserProvidedPeers(
            addrs,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def bind_metrics(
        self,
        *,
        addr: str | None = None,
    ):
        request = Request_SessionBindMetrics(
            addr,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def bind_network(
        self,
        *,
        addrs: list[str],
    ):
        request = Request_SessionBindNetwork(
            addrs,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def bind_remote_control(
        self,
        *,
        addr: str | None = None,
    ) -> "int":
        request = Request_SessionBindRemoteControl(
            addr,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_U16):
            return response.value
        raise UnexpectedResponse()

    async def copy(
        self,
        *,
        src_repo: str | None = None,
        src_path: str,
        dst_repo: str | None = None,
        dst_path: str,
    ):
        request = Request_SessionCopy(
            src_repo,
            src_path,
            dst_repo,
            dst_path,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def create_repository(
        self,
        *,
        path: str,
        read_secret: SetLocalSecret | None = None,
        write_secret: SetLocalSecret | None = None,
        token: str | None = None,
        sync_enabled: bool = False,
        dht_enabled: bool = False,
        pex_enabled: bool = False,
    ) -> "Repository":
        request = Request_SessionCreateRepository(
            path,
            read_secret,
            write_secret,
            token,
            sync_enabled,
            dht_enabled,
            pex_enabled,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Repository):
            return Repository(self._client, response.value)
        raise UnexpectedResponse()

    async def delete_repository_by_name(
        self,
        *,
        name: str,
    ):
        request = Request_SessionDeleteRepositoryByName(
            name,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def derive_secret_key(
        self,
        *,
        password: Password,
        salt: PasswordSalt,
    ) -> "SecretKey":
        request = Request_SessionDeriveSecretKey(
            password,
            salt,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_SecretKey):
            return response.value
        raise UnexpectedResponse()

    async def find_repository(
        self,
        *,
        name: str,
    ) -> "Repository":
        request = Request_SessionFindRepository(
            name,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Repository):
            return Repository(self._client, response.value)
        raise UnexpectedResponse()

    async def generate_password_salt(
        self,
    ) -> "PasswordSalt":
        request = Request_SessionGeneratePasswordSalt()
        response = await self._client.invoke(request)
        if isinstance(response, Response_PasswordSalt):
            return response.value
        raise UnexpectedResponse()

    async def generate_secret_key(
        self,
    ) -> "SecretKey":
        request = Request_SessionGenerateSecretKey()
        response = await self._client.invoke(request)
        if isinstance(response, Response_SecretKey):
            return response.value
        raise UnexpectedResponse()

    async def get_current_protocol_version(
        self,
    ) -> "int":
        request = Request_SessionGetCurrentProtocolVersion()
        response = await self._client.invoke(request)
        if isinstance(response, Response_U64):
            return response.value
        raise UnexpectedResponse()

    async def get_default_block_expiration(
        self,
    ) -> "int | None":
        request = Request_SessionGetDefaultBlockExpiration()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Duration):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_default_quota(
        self,
    ) -> "StorageSize | None":
        request = Request_SessionGetDefaultQuota()
        response = await self._client.invoke(request)
        if isinstance(response, Response_StorageSize):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_default_repository_expiration(
        self,
    ) -> "int | None":
        request = Request_SessionGetDefaultRepositoryExpiration()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Duration):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_dht_routers(
        self,
    ) -> "list[str]":
        request = Request_SessionGetDhtRouters()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Strings):
            return response.value
        raise UnexpectedResponse()

    async def get_external_addr_v4(
        self,
    ) -> "str | None":
        request = Request_SessionGetExternalAddrV4()
        response = await self._client.invoke(request)
        if isinstance(response, Response_SocketAddr):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_external_addr_v6(
        self,
    ) -> "str | None":
        request = Request_SessionGetExternalAddrV6()
        response = await self._client.invoke(request)
        if isinstance(response, Response_SocketAddr):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_highest_seen_protocol_version(
        self,
    ) -> "int":
        request = Request_SessionGetHighestSeenProtocolVersion()
        response = await self._client.invoke(request)
        if isinstance(response, Response_U64):
            return response.value
        raise UnexpectedResponse()

    async def get_local_listener_addrs(
        self,
    ) -> "list[str]":
        request = Request_SessionGetLocalListenerAddrs()
        response = await self._client.invoke(request)
        if isinstance(response, Response_PeerAddrs):
            return response.value
        raise UnexpectedResponse()

    async def get_metrics_listener_addr(
        self,
    ) -> "str | None":
        request = Request_SessionGetMetricsListenerAddr()
        response = await self._client.invoke(request)
        if isinstance(response, Response_SocketAddr):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_mount_root(
        self,
    ) -> "str | None":
        request = Request_SessionGetMountRoot()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Path):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_nat_behavior(
        self,
    ) -> "NatBehavior | None":
        request = Request_SessionGetNatBehavior()
        response = await self._client.invoke(request)
        if isinstance(response, Response_NatBehavior):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_network_stats(
        self,
    ) -> "Stats":
        request = Request_SessionGetNetworkStats()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Stats):
            return response.value
        raise UnexpectedResponse()

    async def get_peers(
        self,
    ) -> "list[PeerInfo]":
        request = Request_SessionGetPeers()
        response = await self._client.invoke(request)
        if isinstance(response, Response_PeerInfos):
            return response.value
        raise UnexpectedResponse()

    async def get_remote_control_listener_addr(
        self,
    ) -> "str | None":
        request = Request_SessionGetRemoteControlListenerAddr()
        response = await self._client.invoke(request)
        if isinstance(response, Response_SocketAddr):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_remote_listener_addrs(
        self,
        *,
        host: str,
    ) -> "list[str]":
        request = Request_SessionGetRemoteListenerAddrs(
            host,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_PeerAddrs):
            return response.value
        raise UnexpectedResponse()

    async def get_runtime_id(
        self,
    ) -> "PublicRuntimeId":
        request = Request_SessionGetRuntimeId()
        response = await self._client.invoke(request)
        if isinstance(response, Response_PublicRuntimeId):
            return response.value
        raise UnexpectedResponse()

    async def get_share_token_access_mode(
        self,
        *,
        token: str,
    ) -> "AccessMode":
        request = Request_SessionGetShareTokenAccessMode(
            token,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_AccessMode):
            return response.value
        raise UnexpectedResponse()

    async def get_share_token_info_hash(
        self,
        *,
        token: str,
    ) -> "str":
        request = Request_SessionGetShareTokenInfoHash(
            token,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_String):
            return response.value
        raise UnexpectedResponse()

    async def get_share_token_suggested_name(
        self,
        *,
        token: str,
    ) -> "str":
        request = Request_SessionGetShareTokenSuggestedName(
            token,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_String):
            return response.value
        raise UnexpectedResponse()

    async def get_state_monitor(
        self,
        *,
        path: list[MonitorId],
    ) -> "typing.Any | None":
        request = Request_SessionGetStateMonitor(
            path,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_StateMonitor):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_store_dirs(
        self,
    ) -> "list[str]":
        request = Request_SessionGetStoreDirs()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Paths):
            return response.value
        raise UnexpectedResponse()

    async def get_user_provided_peers(
        self,
    ) -> "list[str]":
        request = Request_SessionGetUserProvidedPeers()
        response = await self._client.invoke(request)
        if isinstance(response, Response_PeerAddrs):
            return response.value
        raise UnexpectedResponse()

    async def init_network(
        self,
        *,
        defaults: NetworkDefaults,
    ):
        request = Request_SessionInitNetwork(
            defaults,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def insert_store_dirs(
        self,
        *,
        paths: list[str],
    ):
        request = Request_SessionInsertStoreDirs(
            paths,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def is_local_dht_enabled(
        self,
    ) -> "bool":
        request = Request_SessionIsLocalDhtEnabled()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bool):
            return response.value
        raise UnexpectedResponse()

    async def is_local_discovery_enabled(
        self,
    ) -> "bool":
        request = Request_SessionIsLocalDiscoveryEnabled()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bool):
            return response.value
        raise UnexpectedResponse()

    async def is_pex_recv_enabled(
        self,
    ) -> "bool":
        request = Request_SessionIsPexRecvEnabled()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bool):
            return response.value
        raise UnexpectedResponse()

    async def is_pex_send_enabled(
        self,
    ) -> "bool":
        request = Request_SessionIsPexSendEnabled()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bool):
            return response.value
        raise UnexpectedResponse()

    async def is_port_forwarding_enabled(
        self,
    ) -> "bool":
        request = Request_SessionIsPortForwardingEnabled()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bool):
            return response.value
        raise UnexpectedResponse()

    async def list_repositories(
        self,
    ) -> "dict[str, Repository]":
        request = Request_SessionListRepositories()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Repositories):
            return {k: Repository(self._client, v) for k, v in response.value.items()}
        raise UnexpectedResponse()

    async def mirror_exists(
        self,
        *,
        token: str,
        host: str,
    ) -> "bool":
        request = Request_SessionMirrorExists(
            token,
            host,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bool):
            return response.value
        raise UnexpectedResponse()

    async def open_network_socket_v4(
        self,
    ) -> "NetworkSocket | None":
        request = Request_SessionOpenNetworkSocketV4()
        response = await self._client.invoke(request)
        if isinstance(response, Response_NetworkSocket):
            return NetworkSocket(self._client, response.value)
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def open_network_socket_v6(
        self,
    ) -> "NetworkSocket | None":
        request = Request_SessionOpenNetworkSocketV6()
        response = await self._client.invoke(request)
        if isinstance(response, Response_NetworkSocket):
            return NetworkSocket(self._client, response.value)
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def open_network_stream(
        self,
        *,
        addr: str,
        topic_id: TopicId,
    ) -> "NetworkStream":
        request = Request_SessionOpenNetworkStream(
            addr,
            topic_id,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_NetworkStream):
            return NetworkStream(self._client, response.value)
        raise UnexpectedResponse()

    async def open_repository(
        self,
        *,
        path: str,
        local_secret: LocalSecret | None = None,
    ) -> "Repository":
        request = Request_SessionOpenRepository(
            path,
            local_secret,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Repository):
            return Repository(self._client, response.value)
        raise UnexpectedResponse()

    async def pin_dht(
        self,
    ):
        request = Request_SessionPinDht()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def remove_store_dirs(
        self,
        *,
        paths: list[str],
    ):
        request = Request_SessionRemoveStoreDirs(
            paths,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def remove_user_provided_peers(
        self,
        *,
        addrs: list[str],
    ):
        request = Request_SessionRemoveUserProvidedPeers(
            addrs,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_default_block_expiration(
        self,
        *,
        value: int | None = None,
    ):
        request = Request_SessionSetDefaultBlockExpiration(
            value,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_default_quota(
        self,
        *,
        value: StorageSize | None = None,
    ):
        request = Request_SessionSetDefaultQuota(
            value,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_default_repository_expiration(
        self,
        *,
        value: int | None = None,
    ):
        request = Request_SessionSetDefaultRepositoryExpiration(
            value,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_dht_routers(
        self,
        *,
        routers: list[str],
    ):
        request = Request_SessionSetDhtRouters(
            routers,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_local_dht_enabled(
        self,
        *,
        enabled: bool = False,
    ):
        request = Request_SessionSetLocalDhtEnabled(
            enabled,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_local_discovery_enabled(
        self,
        *,
        enabled: bool = False,
    ):
        request = Request_SessionSetLocalDiscoveryEnabled(
            enabled,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_mount_root(
        self,
        *,
        path: str | None = None,
    ):
        request = Request_SessionSetMountRoot(
            path,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_pex_recv_enabled(
        self,
        *,
        enabled: bool = False,
    ):
        request = Request_SessionSetPexRecvEnabled(
            enabled,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_pex_send_enabled(
        self,
        *,
        enabled: bool = False,
    ):
        request = Request_SessionSetPexSendEnabled(
            enabled,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_port_forwarding_enabled(
        self,
        *,
        enabled: bool = False,
    ):
        request = Request_SessionSetPortForwardingEnabled(
            enabled,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_store_dirs(
        self,
        *,
        paths: list[str],
    ):
        request = Request_SessionSetStoreDirs(
            paths,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def unpin_dht(
        self,
    ):
        request = Request_SessionUnpinDht()
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def validate_share_token(
        self,
        *,
        token: str,
    ) -> "str":
        request = Request_SessionValidateShareToken(
            token,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_ShareToken):
            return response.value
        raise UnexpectedResponse()


class Repository:
    def __init__(self, client: "Client", handle: "RepositoryHandle"):
        self._client = client
        self._handle = handle

    async def close(
        self,
    ):
        request = Request_RepositoryClose(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def create_directory(
        self,
        *,
        path: str,
    ):
        request = Request_RepositoryCreateDirectory(
            self._handle,
            path,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def create_file(
        self,
        *,
        path: str,
    ) -> "File":
        request = Request_RepositoryCreateFile(
            self._handle,
            path,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_File):
            return File(self._client, response.value)
        raise UnexpectedResponse()

    async def create_mirror(
        self,
        *,
        host: str,
    ):
        request = Request_RepositoryCreateMirror(
            self._handle,
            host,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def delete(
        self,
    ):
        request = Request_RepositoryDelete(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def delete_mirror(
        self,
        *,
        host: str,
    ):
        request = Request_RepositoryDeleteMirror(
            self._handle,
            host,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def export(
        self,
        *,
        output_path: str,
    ) -> "str":
        request = Request_RepositoryExport(
            self._handle,
            output_path,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Path):
            return response.value
        raise UnexpectedResponse()

    async def file_exists(
        self,
        *,
        path: str,
    ) -> "bool":
        request = Request_RepositoryFileExists(
            self._handle,
            path,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bool):
            return response.value
        raise UnexpectedResponse()

    async def get_access_mode(
        self,
    ) -> "AccessMode":
        request = Request_RepositoryGetAccessMode(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_AccessMode):
            return response.value
        raise UnexpectedResponse()

    async def get_block_expiration(
        self,
    ) -> "int | None":
        request = Request_RepositoryGetBlockExpiration(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Duration):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_credentials(
        self,
    ) -> "bytes":
        request = Request_RepositoryGetCredentials(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bytes):
            return response.value
        raise UnexpectedResponse()

    async def get_entry_type(
        self,
        *,
        path: str,
    ) -> "EntryType | None":
        request = Request_RepositoryGetEntryType(
            self._handle,
            path,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_EntryType):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_expiration(
        self,
    ) -> "int | None":
        request = Request_RepositoryGetExpiration(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Duration):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_info_hash(
        self,
    ) -> "str":
        request = Request_RepositoryGetInfoHash(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_String):
            return response.value
        raise UnexpectedResponse()

    async def get_metadata(
        self,
        *,
        key: str,
    ) -> "str | None":
        request = Request_RepositoryGetMetadata(
            self._handle,
            key,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_String):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_mount_point(
        self,
    ) -> "str | None":
        request = Request_RepositoryGetMountPoint(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Path):
            return response.value
        if isinstance(response, Response_None):
            return None
        raise UnexpectedResponse()

    async def get_path(
        self,
    ) -> "str":
        request = Request_RepositoryGetPath(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Path):
            return response.value
        raise UnexpectedResponse()

    async def get_quota(
        self,
    ) -> "QuotaInfo":
        request = Request_RepositoryGetQuota(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_QuotaInfo):
            return response.value
        raise UnexpectedResponse()

    async def get_short_name(
        self,
    ) -> "str":
        request = Request_RepositoryGetShortName(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_String):
            return response.value
        raise UnexpectedResponse()

    async def get_stats(
        self,
    ) -> "Stats":
        request = Request_RepositoryGetStats(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Stats):
            return response.value
        raise UnexpectedResponse()

    async def get_sync_progress(
        self,
    ) -> "Progress":
        request = Request_RepositoryGetSyncProgress(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Progress):
            return response.value
        raise UnexpectedResponse()

    async def is_dht_enabled(
        self,
    ) -> "bool":
        request = Request_RepositoryIsDhtEnabled(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bool):
            return response.value
        raise UnexpectedResponse()

    async def is_pex_enabled(
        self,
    ) -> "bool":
        request = Request_RepositoryIsPexEnabled(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bool):
            return response.value
        raise UnexpectedResponse()

    async def is_sync_enabled(
        self,
    ) -> "bool":
        request = Request_RepositoryIsSyncEnabled(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bool):
            return response.value
        raise UnexpectedResponse()

    async def mirror_exists(
        self,
        *,
        host: str,
    ) -> "bool":
        request = Request_RepositoryMirrorExists(
            self._handle,
            host,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bool):
            return response.value
        raise UnexpectedResponse()

    async def mount(
        self,
    ) -> "str":
        request = Request_RepositoryMount(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Path):
            return response.value
        raise UnexpectedResponse()

    async def move(
        self,
        *,
        dst: str,
    ):
        request = Request_RepositoryMove(
            self._handle,
            dst,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def move_entry(
        self,
        *,
        src: str,
        dst: str,
    ):
        request = Request_RepositoryMoveEntry(
            self._handle,
            src,
            dst,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def open_file(
        self,
        *,
        path: str,
    ) -> "File":
        request = Request_RepositoryOpenFile(
            self._handle,
            path,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_File):
            return File(self._client, response.value)
        raise UnexpectedResponse()

    async def read_directory(
        self,
        *,
        path: str,
    ) -> "list[DirectoryEntry]":
        request = Request_RepositoryReadDirectory(
            self._handle,
            path,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_DirectoryEntries):
            return response.value
        raise UnexpectedResponse()

    async def remove_directory(
        self,
        *,
        path: str,
        recursive: bool = False,
    ):
        request = Request_RepositoryRemoveDirectory(
            self._handle,
            path,
            recursive,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def remove_file(
        self,
        *,
        path: str,
    ):
        request = Request_RepositoryRemoveFile(
            self._handle,
            path,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def reset_access(
        self,
        *,
        token: str,
    ):
        request = Request_RepositoryResetAccess(
            self._handle,
            token,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_access(
        self,
        *,
        read: AccessChange | None = None,
        write: AccessChange | None = None,
    ):
        request = Request_RepositorySetAccess(
            self._handle,
            read,
            write,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_access_mode(
        self,
        *,
        access_mode: AccessMode,
        local_secret: LocalSecret | None = None,
    ):
        request = Request_RepositorySetAccessMode(
            self._handle,
            access_mode,
            local_secret,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_block_expiration(
        self,
        *,
        value: int | None = None,
    ):
        request = Request_RepositorySetBlockExpiration(
            self._handle,
            value,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_credentials(
        self,
        *,
        credentials: bytes,
    ):
        request = Request_RepositorySetCredentials(
            self._handle,
            credentials,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_dht_enabled(
        self,
        *,
        enabled: bool = False,
    ):
        request = Request_RepositorySetDhtEnabled(
            self._handle,
            enabled,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_expiration(
        self,
        *,
        value: int | None = None,
    ):
        request = Request_RepositorySetExpiration(
            self._handle,
            value,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_metadata(
        self,
        *,
        edits: list[MetadataEdit],
    ) -> "bool":
        request = Request_RepositorySetMetadata(
            self._handle,
            edits,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bool):
            return response.value
        raise UnexpectedResponse()

    async def set_pex_enabled(
        self,
        *,
        enabled: bool = False,
    ):
        request = Request_RepositorySetPexEnabled(
            self._handle,
            enabled,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_quota(
        self,
        *,
        value: StorageSize | None = None,
    ):
        request = Request_RepositorySetQuota(
            self._handle,
            value,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def set_sync_enabled(
        self,
        *,
        enabled: bool = False,
    ):
        request = Request_RepositorySetSyncEnabled(
            self._handle,
            enabled,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def share(
        self,
        *,
        access_mode: AccessMode,
        local_secret: LocalSecret | None = None,
    ) -> "str":
        request = Request_RepositoryShare(
            self._handle,
            access_mode,
            local_secret,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_ShareToken):
            return response.value
        raise UnexpectedResponse()

    async def unmount(
        self,
    ):
        request = Request_RepositoryUnmount(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()


class File:
    def __init__(self, client: "Client", handle: "FileHandle"):
        self._client = client
        self._handle = handle

    async def close(
        self,
    ):
        request = Request_FileClose(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def flush(
        self,
    ):
        request = Request_FileFlush(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def get_length(
        self,
    ) -> "int":
        request = Request_FileGetLength(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_U64):
            return response.value
        raise UnexpectedResponse()

    async def get_progress(
        self,
    ) -> "int":
        request = Request_FileGetProgress(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_U64):
            return response.value
        raise UnexpectedResponse()

    async def read(
        self,
        *,
        offset: int,
        size: int,
    ) -> "bytes":
        request = Request_FileRead(
            self._handle,
            offset,
            size,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bytes):
            return response.value
        raise UnexpectedResponse()

    async def truncate(
        self,
        *,
        len: int,
    ):
        request = Request_FileTruncate(
            self._handle,
            len,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def write(
        self,
        *,
        offset: int,
        data: bytes,
    ):
        request = Request_FileWrite(
            self._handle,
            offset,
            data,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()


class NetworkSocket:
    def __init__(self, client: "Client", handle: "NetworkSocketHandle"):
        self._client = client
        self._handle = handle

    async def close(
        self,
    ):
        request = Request_NetworkSocketClose(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def recv_from(
        self,
        *,
        len: int,
    ) -> "Datagram":
        request = Request_NetworkSocketRecvFrom(
            self._handle,
            len,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Datagram):
            return response.value
        raise UnexpectedResponse()

    async def send_to(
        self,
        *,
        data: bytes,
        addr: str,
    ) -> "int":
        request = Request_NetworkSocketSendTo(
            self._handle,
            data,
            addr,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_U64):
            return response.value
        raise UnexpectedResponse()


class NetworkStream:
    def __init__(self, client: "Client", handle: "NetworkStreamHandle"):
        self._client = client
        self._handle = handle

    async def close(
        self,
    ):
        request = Request_NetworkStreamClose(
            self._handle,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()

    async def read_exact(
        self,
        *,
        len: int,
    ) -> "bytes":
        request = Request_NetworkStreamReadExact(
            self._handle,
            len,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Bytes):
            return response.value
        raise UnexpectedResponse()

    async def write_all(
        self,
        *,
        buf: bytes,
    ):
        request = Request_NetworkStreamWriteAll(
            self._handle,
            buf,
        )
        response = await self._client.invoke(request)
        if isinstance(response, Response_Unit):
            return
        raise UnexpectedResponse()


class UnexpectedResponse(OuisyncError):
    def __init__(self):
        super().__init__(ErrorCode.INVALID_DATA, "unexpected response")

