package org.equalitie.ouisync

class Repository private constructor(private val handle: Long, private val client: Client) {
    companion object {
        suspend fun create(
            session: Session,
            path: String,
            readPassword: String?,
            writePassword: String?,
            shareToken: String? = null,
        ): Repository {
            val client = session.client
            val handle = client.invoke(
                RepositoryCreate(
                    path,
                    readPassword,
                    writePassword,
                    shareToken,
                ),
            ) as Long

            return Repository(handle, client)
        }

        suspend fun open(session: Session, path: String, password: String? = null): Repository {
            val client = session.client
            val handle = client.invoke(RepositoryOpen(path, password)) as Long

            return Repository(handle, client)
        }

        suspend fun reopen(session: Session, path: String, token: ByteArray): Repository {
            val client = session.client
            val handle = client.invoke(RepositoryReopen(path, token)) as Long

            return Repository(handle, client)
        }
    }

    suspend fun close() = client.invoke(RepositoryClose(handle))

    suspend fun subscribe(): EventReceiver<Unit> =
        client.subscribe(RepositorySubscribe(handle))

    suspend fun infoHash() = client.invoke(RepositoryInfoHash(handle)) as String

    suspend fun databaseId() = client.invoke(RepositoryDatabaseId(handle)) as ByteArray

    suspend fun accessMode(): AccessMode {
        val raw = client.invoke(RepositoryAccessMode(handle)) as Byte
        return AccessMode.decode(raw)
    }

    suspend fun createShareToken(
        password: String? = null,
        accessMode: AccessMode = AccessMode.WRITE,
        name: String? = null,
    ) = client.invoke(RepositoryCreateShareToken(handle, password, accessMode, name)) as String

    suspend fun createReopenToken() =
        client.invoke(RepositoryCreateReopenToken(handle)) as ByteArray

    suspend fun requiresLocalPasswordForReading() =
        client.invoke(RepositoryRequiresLocalPasswordForReading(handle)) as Boolean

    suspend fun requiresLocalPasswordForWriting() =
        client.invoke(RepositoryRequiresLocalPasswordForWriting(handle)) as Boolean

    suspend fun setReadAccess(password: String?, shareToken: String? = null) =
        client.invoke(RepositorySetReadAccess(handle, password, shareToken))

    suspend fun setReadAndWriteAccess(oldPassword: String?, newPassword: String?, shareToken: String? = null) =
        client.invoke(RepositorySetReadAndWriteAccess(handle, oldPassword, newPassword, shareToken))

    suspend fun removeReadKey() = client.invoke(RepositoryRemoveReadKey(handle))

    suspend fun removeWriteKey() = client.invoke(RepositoryRemoveWriteKey(handle))

    suspend fun isDhtEnabled() = client.invoke(RepositoryIsDhtEnabled(handle)) as Boolean

    suspend fun setDhtEnabled(enabled: Boolean) =
        client.invoke(RepositorySetDhtEnabled(handle, enabled))

    suspend fun isPexEnabled() = client.invoke(RepositoryIsPexEnabled(handle)) as Boolean

    suspend fun setPexEnabled(enabled: Boolean) =
        client.invoke(RepositorySetPexEnabled(handle, enabled))

    /**
     * Returns the synchronization progress of this repository
     */
    suspend fun syncProgress() = client.invoke(RepositorySyncProgress(handle)) as Progress

    /**
     * Create mirror of this repository on server(s) that were previously added with
     * [Session.addStorageServer].
     */
    suspend fun mirror() = client.invoke(RepositoryMirror(handle))

    /**
     * Returns the type (file or directory) of the entry at the given path, or `null` if no such
     * entry exists.
     */
    suspend fun entryType(path: String): EntryType? {
        val raw = client.invoke(RepositoryEntryType(handle, path)) as Byte?

        if (raw != null) {
            return EntryType.decode(raw)
        } else {
            return null
        }
    }
}
