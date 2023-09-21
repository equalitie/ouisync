package org.equalitie.ouisync

import org.msgpack.core.MessagePacker

internal sealed class Request {
    fun pack(packer: MessagePacker) {
        packer.packMapHeader(1)
        packer.packString(name())
        packContent(packer)
        packer.close()
    }

    protected abstract fun packContent(packer: MessagePacker)

    private fun name(): String {
        return this::class.simpleName!!.toSnakeCase()
    }
}

private fun String.stripSuffix(suffix: String): String = when {
    endsWith(suffix) -> substring(0, length - suffix.length)
    else -> this
}

private fun String.toSnakeCase(): String {
    val builder = StringBuilder()

    builder.append(this[0].lowercase())

    for (c in drop(1)) {
        if (c.isUpperCase()) {
            builder.append('_').append(c.lowercase())
        } else {
            builder.append(c)
        }
    }

    return builder.toString()
}

internal open class EmptyRequest : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packNil()
    }
}

internal open class BooleanRequest(val value: Boolean) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packBoolean(value)
    }
}

internal open class LongRequest(val value: Long) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packLong(value)
    }
}

internal open class StringRequest(val value: String) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packString(value)
    }
}

/*
    RepositoryCreate {
        path: Utf8PathBuf,
        read_password: Option<String>,
        write_password: Option<String>,
        share_token: Option<ShareToken>,
    },
    RepositoryOpen {
        path: Utf8PathBuf,
        password: Option<String>,
    },
    RepositoryClose(Handle<RepositoryHolder>),
    RepositoryCreateReopenToken(Handle<RepositoryHolder>),
    RepositoryReopen {
        path: Utf8PathBuf,
        #[serde(with = "serde_bytes")]
        token: Vec<u8>,
    },
    RepositorySubscribe(Handle<RepositoryHolder>),
    RepositorySetReadAccess {
        repository: Handle<RepositoryHolder>,
        password: Option<String>,
        share_token: Option<ShareToken>,
    },
    RepositorySetReadAndWriteAccess {
        repository: Handle<RepositoryHolder>,
        old_password: Option<String>,
        new_password: Option<String>,
        share_token: Option<ShareToken>,
    },
    RepositoryRemoveReadKey(Handle<RepositoryHolder>),
    RepositoryRemoveWriteKey(Handle<RepositoryHolder>),
    RepositoryRequiresLocalPasswordForReading(Handle<RepositoryHolder>),
    RepositoryRequiresLocalPasswordForWriting(Handle<RepositoryHolder>),
    RepositoryInfoHash(Handle<RepositoryHolder>),
    RepositoryDatabaseId(Handle<RepositoryHolder>),
    RepositoryEntryType {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
    },
    RepositoryMoveEntry {
        repository: Handle<RepositoryHolder>,
        src: Utf8PathBuf,
        dst: Utf8PathBuf,
    },
    RepositoryIsDhtEnabled(Handle<RepositoryHolder>),
    RepositorySetDhtEnabled {
        repository: Handle<RepositoryHolder>,
        enabled: bool,
    },
    RepositoryIsPexEnabled(Handle<RepositoryHolder>),
    RepositorySetPexEnabled {
        repository: Handle<RepositoryHolder>,
        enabled: bool,
    },
    RepositoryCreateShareToken {
        repository: Handle<RepositoryHolder>,
        password: Option<String>,
        access_mode: AccessMode,
        name: Option<String>,
    },
    RepositoryAccessMode(Handle<RepositoryHolder>),
    RepositorySyncProgress(Handle<RepositoryHolder>),
    RepositoryMirror {
        repository: Handle<RepositoryHolder>,
    },
    RepositoryMountAll(PathBuf),
    ShareTokenMode(#[serde(with = "as_str")] ShareToken),
    ShareTokenInfoHash(#[serde(with = "as_str")] ShareToken),
    ShareTokenSuggestedName(#[serde(with = "as_str")] ShareToken),
    ShareTokenNormalize(#[serde(with = "as_str")] ShareToken),
    DirectoryCreate {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
    },
    DirectoryOpen {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
    },
    DirectoryRemove {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
        recursive: bool,
    },
    FileOpen {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
    },
    FileCreate {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
    },
    FileRemove {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
    },
    FileRead {
        file: Handle<FileHolder>,
        offset: u64,
        len: u64,
    },
    FileWrite {
        file: Handle<FileHolder>,
        offset: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    FileTruncate {
        file: Handle<FileHolder>,
        len: u64,
    },
    FileLen(Handle<FileHolder>),
    FileProgress(Handle<FileHolder>),
    FileFlush(Handle<FileHolder>),
    FileClose(Handle<FileHolder>),
*/

internal data class NetworkInit(
    val portForwardingEnabled: Boolean,
    val localDiscoveryEnabled: Boolean,
) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packMapHeader(2)

        packer.packString("port_forwarding_enabled")
        packer.packBoolean(portForwardingEnabled)

        packer.packString("local_discovery_enabled")
        packer.packBoolean(localDiscoveryEnabled)
    }
}

internal data class NetworkBind(
    val quicV4: String?,
    val quicV6: String?,
    val tcpV4: String?,
    val tcpV6: String?,
) : Request() {
    override fun packContent(packer: MessagePacker) {
        val entries = arrayOf(
            "quic_v4" to quicV4,
            "quic_v6" to quicV6,
            "tcp_v4" to tcpV4,
            "tcp_v6" to tcpV6,
        ).filter { it.second != null }

        packer.packMapHeader(entries.size)

        for ((key, value) in entries) {
            packer.packString(key)
            packer.packString(value)
        }
    }
}

internal class NetworkSubscribe : EmptyRequest()

internal class NetworkTcpListenerLocalAddrV4 : EmptyRequest()

internal class NetworkTcpListenerLocalAddrV6 : EmptyRequest()

internal class NetworkQuicListenerLocalAddrV4 : EmptyRequest()

internal class NetworkQuicListenerLocalAddrV6 : EmptyRequest()

internal class NetworkAddUserProvidedPeer : StringRequest {
    constructor(value: String) : super(value)
}

internal class NetworkRemoveUserProvidedPeer : StringRequest {
    constructor(value: String) : super(value)
}

internal class NetworkKnownPeers : EmptyRequest()

internal class NetworkThisRuntimeId : EmptyRequest()

internal class NetworkCurrentProtocolVersion : EmptyRequest()

internal class NetworkHighestSeenProtocolVersion : EmptyRequest()

internal class NetworkIsPortForwardingEnabled : EmptyRequest()

internal class NetworkSetPortForwardingEnabled : BooleanRequest {
    constructor(value: Boolean) : super(value)
}

internal class NetworkIsLocalDiscoveryEnabled : EmptyRequest()

internal class NetworkSetLocalDiscoveryEnabled : BooleanRequest {
    constructor(value: Boolean) : super(value)
}

internal class NetworkAddStorageServer : StringRequest {
    constructor(value: String) : super(value)
}

internal class NetworkShutdown : EmptyRequest()

internal class Unsubscribe : LongRequest {
    constructor(value: Long) : super(value)
}

/*
    StateMonitorGet(Vec<MonitorId>),
    StateMonitorSubscribe(Vec<MonitorId>),
*/
