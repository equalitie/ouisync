package org.equalitie.ouisync.lib

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

internal class RepositoryGetStoreDir : EmptyRequest()
internal class RepositorySetStoreDir(value: String) : ValueRequest<String>(value)

internal class RepositoryList : EmptyRequest()

internal class RepositoryCreate(
    val path: String,
    val readSecret: SetLocalSecret?,
    val writeSecret: SetLocalSecret?,
    val token: String?,
    val syncEnabled: Boolean,
    val dhtEnabled: Boolean,
    val pexEnabled: Boolean,
) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packMap(
            mapOf(
                "path" to path,
                "read_secret" to readSecret,
                "write_secret" to writeSecret,
                "token" to token,
                "sync_enabled" to syncEnabled,
                "dht_enabled" to dhtEnabled,
                "pex_enabled" to pexEnabled,
            ),
        )
    }
}

internal class RepositoryOpen(val path: String, val secret: LocalSecret?) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packMap(mapOf("path" to path, "secret" to secret))
    }
}

internal class RepositoryClose(value: Long) : ValueRequest<Long>(value)

internal class RepositoryDelete(value: Long) : ValueRequest<Long>(value)

internal class RepositorySubscribe : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositoryIsSyncEnabled : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositorySetSyncEnabled(val repository: Long, val enabled: Boolean) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packMap(
            mapOf(
                "repository" to repository,
                "enabled" to enabled,
            ),
        )
    }
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

internal class RepositoryGetAccessMode : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositorySetAccessMode(
    val repository: Long,
    val mode: AccessMode,
    val secret: LocalSecret?,
) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packMap(
            mapOf(
                "repository" to repository,
                "mode" to mode,
                "secret" to secret,
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

internal class RepositoryGetInfoHash(value: Long) : ValueRequest<Long>(value)

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

internal class RepositoryShare(
    val repository: Long,
    val secret: LocalSecret?,
    val mode: AccessMode,
) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(
            mapOf(
                "repository" to repository,
                "secret" to secret,
                "mode" to mode.encode(),
            ),
        )
}

internal class RepositorySyncProgress : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal class RepositoryCreateMirror(val repository: Long, val host: String) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(
            mapOf(
                "repository" to repository,
                "host" to host,
            ),
        )
}

internal class RepositoryDeleteMirror(val repository: Long, val host: String) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(
            mapOf(
                "repository" to repository,
                "host" to host,
            ),
        )
}

internal class RepositoryMirrorExists(val repository: Long, val host: String) : Request() {
    override fun packContent(packer: MessagePacker) =
        packer.packMap(
            mapOf(
                "repository" to repository,
                "host" to host,
            ),
        )
}

internal class ShareTokenGetAccessMode : ValueRequest<String> {
    constructor(value: String) : super(value)
}

internal class ShareTokenGetInfoHash : ValueRequest<String> {
    constructor(value: String) : super(value)
}

internal class ShareTokenGetSuggestedName : ValueRequest<String> {
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

internal class DirectoryRead(val repository: Long, val path: String) : Request() {
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
    val bindAddrs: List<String>,
    val portForwardingEnabled: Boolean,
    val localDiscoveryEnabled: Boolean,
) : Request() {
    override fun packContent(packer: MessagePacker) = packer.packMap(
        mapOf(
            "bind" to bindAddrs,
            "port_forwarding_enabled" to portForwardingEnabled,
            "local_discovery_enabled" to localDiscoveryEnabled,
        ),
    )
}

internal class NetworkBind(val addrs: List<String>) : Request() {
    override fun packContent(packer: MessagePacker) {
        packer.packArrayHeader(addrs.size)

        for (addr in addrs) {
            packer.packString(addr)
        }
    }
}

internal class NetworkSubscribe : EmptyRequest()

internal class NetworkGetLocalListenerAddrs : EmptyRequest()

internal class NetworkGetRemoteListenerAddrs(host: String) : ValueRequest<String>(host)

internal class NetworkAddUserProvidedPeers : ValueRequest<List<String>> {
    constructor(value: List<String>) : super(value)
}

internal class NetworkRemoveUserProvidedPeers : ValueRequest<List<String>> {
    constructor(value: List<String>) : super(value)
}

internal class NetworkGetPeers : EmptyRequest()

internal class NetworkGetRuntimeId : EmptyRequest()

internal class NetworkGetCurrentProtocolVersion : EmptyRequest()

internal class NetworkGetHighestSeenProtocolVersion : EmptyRequest()

internal class NetworkIsPortForwardingEnabled : EmptyRequest()

internal class NetworkSetPortForwardingEnabled : ValueRequest<Boolean> {
    constructor(value: Boolean) : super(value)
}

internal class NetworkIsLocalDiscoveryEnabled : EmptyRequest()

internal class NetworkSetLocalDiscoveryEnabled : ValueRequest<Boolean> {
    constructor(value: Boolean) : super(value)
}

internal class Unsubscribe : ValueRequest<Long> {
    constructor(value: Long) : super(value)
}

internal fun MessagePacker.packList(list: List<Any?>) {
    packArrayHeader(list.size)

    for (item in list) {
        packAny(item)
    }
}

internal fun MessagePacker.packMap(map: Map<String, Any?>) {
    packMapHeader(map.count { it.value != null })

    for ((key, value) in map) {
        if (value == null) {
            continue
        }

        packString(key)
        packAny(value)
    }
}

private fun MessagePacker.packAny(value: Any?) {
    when (value) {
        is List<Any?> -> packList(value)
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
        is AccessMode -> packByte(value.encode())
        is LocalSecret -> value.pack(this)
        is SetLocalSecret -> value.pack(this)
        null -> packNil()
        else -> throw IllegalArgumentException("can't pack ${value::class.qualifiedName}")
    }
}
