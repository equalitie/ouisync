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

internal abstract class EmptyRequest : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packNil()
    }
}

internal abstract class ValueRequest<T : Any>(val value: T) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packAny(value)
    }
}

internal class RepositoryCreate(
    val path: String,
    val readPassword: String?,
    val writePassword: String?,
    val shareToken: String?,
) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packMap(
            mapOf(
                "path" to path,
                "readPassword" to readPassword,
                "writePassword" to writePassword,
                "shareToken" to shareToken,
            ),
        )
    }
}

internal class RepositoryOpen(val path: String, val password: String?) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packMap(mapOf("path" to path, "password" to password))
    }
}

internal class RepositoryClose : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositorySubscribe : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositorySetAccess(
    val repository: Long,
    val read: AccessChange?,
    val write: AccessChange?,
) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packMap(
            mapOf(
                "repository" to repository,
                "read" to read,
                "write" to write,
            ),
        )
    }
}

internal class RepositoryAccessMode : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositorySetAccessMode(
    val repository: Long,
    val accessMode: AccessMode,
    val password: String?,
) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packMap(
            mapOf(
                "repository" to repository,
                "access_mode" to accessMode,
                "password" to password,
            ),
        )
    }
}

internal class RepositoryCredentials : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositorySetCredentials(
    val repository: Long,
    val credentials: ByteArray,
) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packMap(
            mapOf(
                "repository" to repository,
                "credentials" to credentials,
            ),
        )
    }
}

internal class RepositoryRemoveReadKey : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositoryRemoveWriteKey : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositoryRequiresLocalPasswordForReading : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositoryRequiresLocalPasswordForWriting : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositoryInfoHash : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositoryDatabaseId : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositoryEntryType(val repository: Long, val path: String) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(
            mapOf(
                "repository" to repository,
                "path" to path,
            ),
        )
}

internal class RepositoryMoveEntry(val repository: Long, val src: String, val dst: String) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(
            mapOf(
                "repository" to repository,
                "src" to src,
                "dst" to dst,
            ),
        )
}

internal class RepositoryIsDhtEnabled : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositorySetDhtEnabled(val repository: Long, val enabled: Boolean) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(mapOf("repository" to repository, "enabled" to enabled))
}

internal class RepositoryIsPexEnabled : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositorySetPexEnabled(val repository: Long, val enabled: Boolean) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(mapOf("repository" to repository, "enabled" to enabled))
}

internal class RepositoryCreateShareToken(
    val repository: Long,
    val password: String?,
    val accessMode: AccessMode,
    val name: String?,
) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(
            mapOf(
                "repository" to repository,
                "password" to password,
                "access_mode" to accessMode.encode(),
                "name" to name,
            ),
        )
}

internal class RepositorySyncProgress : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositoryMirror : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositoryMountAll : ValueRequest<String> {
    constructor(value: String) : super(value)
}

internal class ShareTokenMode : ValueRequest<String> {
    constructor(value: String) : super(value)
}

internal class ShareTokenInfoHash : ValueRequest<String> {
    constructor(value: String) : super(value)
}

internal class ShareTokenSuggestedName : ValueRequest<String> {
    constructor(value: String) : super(value)
}

internal class ShareTokenNormalize : ValueRequest<String> {
    constructor(value: String) : super(value)
}

internal class DirectoryCreate(val repository: Long, val path: String) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(
            mapOf(
                "repository" to repository,
                "path" to path,
            ),
        )
}

internal class DirectoryOpen(val repository: Long, val path: String) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(
            mapOf(
                "repository" to repository,
                "path" to path,
            ),
        )
}

internal class DirectoryRemove(val repository: Long, val path: String, val recursive: Boolean) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(
            mapOf(
                "repository" to repository,
                "path" to path,
                "recursive" to recursive,
            ),
        )
}

internal class FileOpen(val repository: Long, val path: String) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(mapOf("repository" to repository, "path" to path))
}

internal class FileCreate(val repository: Long, val path: String) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(mapOf("repository" to repository, "path" to path))
}

internal class FileRemove(val repository: Long, val path: String) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(mapOf("repository" to repository, "path" to path))
}

internal class FileRead(val file: Long, val offset: Long, val len: Long) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(mapOf("file" to file, "offset" to offset, "len" to len))
}

internal class FileWrite(val file: Long, val offset: Long, val data: ByteArray) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(mapOf("file" to file, "offset" to offset, "data" to data))
}

internal class FileTruncate(val file: Long, val len: Long) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(mapOf("file" to file, "len" to len))
}

internal class FileLen : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class FileProgress : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class FileFlush : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class FileClose : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class NetworkInit(
    val portForwardingEnabled: Boolean,
    val localDiscoveryEnabled: Boolean,
) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(
            mapOf(
                "port_forwarding_enabled" to
                    portForwardingEnabled,
                "local_discovery_enabled" to
                    localDiscoveryEnabled,
            ),
        )
}

internal class NetworkBind(
    val quicV4: String?,
    val quicV6: String?,
    val tcpV4: String?,
    val tcpV6: String?,
) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packMap(
            mapOf(
                "quic_v4" to quicV4,
                "quic_v6" to quicV6,
                "tcp_v4" to tcpV4,
                "tcp_v6" to tcpV6,
            ),
        )
    }
}

internal class NetworkSubscribe : EmptyRequest()

internal class NetworkTcpListenerLocalAddrV4 : EmptyRequest()

internal class NetworkTcpListenerLocalAddrV6 : EmptyRequest()

internal class NetworkQuicListenerLocalAddrV4 : EmptyRequest()

internal class NetworkQuicListenerLocalAddrV6 : EmptyRequest()

internal class NetworkAddUserProvidedPeer : ValueRequest<String> {
    constructor(value: String) : super(value)
}

internal class NetworkRemoveUserProvidedPeer : ValueRequest<String> {
    constructor(value: String) : super(value)
}

internal class NetworkKnownPeers : EmptyRequest()

internal class NetworkThisRuntimeId : EmptyRequest()

internal class NetworkCurrentProtocolVersion : EmptyRequest()

internal class NetworkHighestSeenProtocolVersion : EmptyRequest()

internal class NetworkIsPortForwardingEnabled : EmptyRequest()

internal class NetworkSetPortForwardingEnabled : ValueRequest<Boolean> {
    constructor(value: Boolean) : super(value)
}

internal class NetworkIsLocalDiscoveryEnabled : EmptyRequest()

internal class NetworkSetLocalDiscoveryEnabled : ValueRequest<Boolean> {
    constructor(value: Boolean) : super(value)
}

internal class NetworkAddCacheServer : ValueRequest<String> {
    constructor(value: String) : super(value)
}

internal class NetworkShutdown : EmptyRequest()

internal class Unsubscribe : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

private fun MessagePacker.packMap(map: Map<String, Any?>) {
    packMapHeader(map.count { it.value != null })

    for ((key, value) in map) {
        if (value == null) {
            continue
        }

        packString(key)
        packAny(value)
    }
}

private fun MessagePacker.packAny(value: Any) {
    when (value) {
        is Boolean -> packBoolean(value)
        is Byte -> packByte(value)
        is ByteArray -> {
            packBinaryHeader(value.size)
            writePayload(value)
        }
        is Int -> packInt(value)
        is Long -> packLong(value)
        is Short -> packShort(value)
        is String -> packString(value)
        is AccessChange -> value.pack(this)
        else -> throw IllegalArgumentException("can't pack ${value::class.qualifiedName}")
    }
}
