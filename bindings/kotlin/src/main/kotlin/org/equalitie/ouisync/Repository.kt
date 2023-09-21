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
    }

    suspend fun close() = client.invoke(RepositoryClose(handle))

    suspend fun subscribe(): EventReceiver<Unit> = client.subscribe(RepositorySubscribe(handle))

    suspend fun infoHash(): String = client.invoke(RepositoryInfoHash(handle)) as String

    suspend fun isDhtEnabled(): Boolean = client.invoke(RepositoryIsDhtEnabled(handle)) as Boolean

    suspend fun setDhtEnabled(enabled: Boolean) = client.invoke(RepositorySetDhtEnabled(handle, enabled))

    suspend fun isPexEnabled(): Boolean = client.invoke(RepositoryIsPexEnabled(handle)) as Boolean

    suspend fun setPexEnabled(enabled: Boolean) = client.invoke(RepositorySetPexEnabled(handle, enabled))

    suspend fun createShareToken(
        password: String? = null,
        accessMode: AccessMode = AccessMode.WRITE,
        name: String? = null,
    ): String = client.invoke(RepositoryCreateShareToken(handle, password, accessMode, name)) as String
}
