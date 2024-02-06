package org.equalitie.ouisync

import org.msgpack.core.MessagePacker

/**
 *
 * A Ouisync repository.
 *
 * Example usage:
 *
 * ```
 * // Create a new repo:
 * val repo = Repository.create(session, "path/to/the/repo.ouisyncdb")
 *
 * // or open an existing one:
 * val repo = Repository.open(session, "path/to/the/repo.ouisyncdb")
 *
 * // Access the repository files (see File, Directory) ...
 *
 * // Close it when done:
 * repo.close()
 * ```
 *
 * To share a repository, create the share token with [createShareToken], send it to the peer
 * (e.g., via a secure instant messenger, encode as QR code and scan, ...), then create a
 * repository on the peer's device with [create], passing the share token to it.
 *
 * @see Session
 * @see File
 * @see Directory
 */
class Repository private constructor(internal val handle: Long, internal val client: Client) {
    companion object {
        /**
         * Creates a new repository.
         *
         * @param session       the Ouisync session.
         * @param path          path to the local file to store the repository in. It's recommended
         *                      to use the "ouisyncdb" file extension, but any extension works.
         * @param readPassword  local password for reading the repository on this device only. Do
         *                      not share with peers!. If null, the repo won't be password
         *                      protected and anyone with physical access to the device will be
         *                      able to read it.
         * @param writePassword local password for writing to the repository on this device only.
         *                      Do not share with peers! Can be the same as `readPassword` if one
         *                      wants to use only one password for both reading and writing.
         *                      Separate passwords are useful for plausible deniability. If both
         *                      `readPassword` and `writePassword` are null, the repo won't be
         *                      password protected and anyone with physical access to the device
         *                      will be able to read and write to it. If `readPassword` is not null
         *                      but `writePassword` is null, the repo won't be writable from this
         *                      device.
         * @param shareToken    used to share repositories between devices. If not null, this repo
         *                      will be linked with the repos with the same share token on other
         *                      devices. See also [createShareToken]. This also determines the
         *                      maximal access mode the repo can be opened in. If null, it's *write*
         *                      mode.
         */
        suspend fun create(
            session: Session,
            path: String,
            readPassword: String?,
            writePassword: String?,
            shareToken: ShareToken? = null,
        ): Repository {
            val client = session.client
            val handle = client.invoke(
                RepositoryCreate(
                    path,
                    readPassword,
                    writePassword,
                    shareToken?.toString(),
                ),
            ) as Long

            return Repository(handle, client)
        }

        /**
         * Open an existing repository.
         *
         * @param session  the Ouisync session.
         * @param path     path to the local file the repo is stored in.
         * @param password a local password. See the `readPassword` and `writePassword` param in
         *                 [create] for more details. If this repo uses local password(s), this
         *                 determines the access mode the repo is opened in: `readPassword` opens it
         *                 in *read* mode, `writePassword` opens it in *write* mode and no password
         *                 or wrong password opens it in *blind* mode. If this repo doesn't use
         *                 local password(s), the repo is opened in the maximal mode specified when
         *                 the repo was created.
         */
        suspend fun open(session: Session, path: String, password: String? = null): Repository {
            val client = session.client
            val handle = client.invoke(RepositoryOpen(path, password)) as Long

            return Repository(handle, client)
        }
    }

    /**
     * Closes the repository.
     */
    suspend fun close() = client.invoke(RepositoryClose(handle))

    /**
     * Subscribe to repository events.
     *
     * An even is emitted every time the content of the repo changes (e.g., a file is created,
     * written to, removed, moved, ...).
     */
    suspend fun subscribe(): EventReceiver<Unit> =
        client.subscribe(RepositorySubscribe(handle))

    /**
     * Returns the info-hash used to announce the repo on the Bittorrent DHT.
     *
     * Note the announce happens automatically. This function exists just for information/debugging
     * purposes.
     */
    suspend fun infoHash() = client.invoke(RepositoryInfoHash(handle)) as String

    /**
     * Returns the *database id* of this repository. A *database id* remains unchanged even when
     * the repo is renamed or moved and so can be used e.g. as a key for storing any per-repo
     * configuration, if needed.
     */
    suspend fun databaseId() = client.invoke(RepositoryDatabaseId(handle)) as ByteArray

    /**
     * Creates a *share token* to share this repository with other devices.
     *
     * By default the access mode of the token will be the same as the mode the repo is currently
     * opened in but it can be escalated with the `password` param or de-escalated with the
     * `accessMode` param.
     *
     * @param password   local repo password. If not null, the share token's access mode will be
     *                   the same as what the password provides. Useful to escalate the access mode
     *                   to above of what the repo is opened in.
     * @param accessMode access mode of the token. Useful to de-escalate the access mode to below
     *                   of what the repo is opened in.
     * @param name       optional human-readable name of the repo that the share token will be
     *                   labeled with. Useful to help organize the share tokens.
     */
    suspend fun createShareToken(
        password: String? = null,
        accessMode: AccessMode = AccessMode.WRITE,
        name: String? = null,
    ): ShareToken {
        val raw = client.invoke(RepositoryCreateShareToken(handle, password, accessMode, name)) as String
        return ShareToken(raw, client)
    }

    /**
     * Is local password required to read this repo?
     */
    suspend fun requiresLocalPasswordForReading() =
        client.invoke(RepositoryRequiresLocalPasswordForReading(handle)) as Boolean

    /**
     * Is local password required to write to this repo?
     */
    suspend fun requiresLocalPasswordForWriting() =
        client.invoke(RepositoryRequiresLocalPasswordForWriting(handle)) as Boolean

    /**
     * Returns the access mode (*blind*, *read* or *write*) the repo is opened in.
     */
    suspend fun accessMode(): AccessMode {
        val raw = client.invoke(RepositoryAccessMode(handle)) as Byte
        return AccessMode.decode(raw)
    }

    /**
     * Switches the repository to the given access mode.
     */
    suspend fun setAccessMode(
        accessMode: AccessMode,
        password: String?,
    ) =
        client.invoke(RepositorySetAccessMode(handle, accessMode, password))

    /**
     * Gets the current credentials of this repository. Can be used to restore access after
     * closing and reopening the repository.
     *
     * @see setCredentials
     */
    suspend fun credentials(): ByteArray = client.invoke(RepositoryCredentials(handle)) as ByteArray

    /**
     * Sets the current credentials of the repository.
     */
    suspend fun setCredentials(credentials: ByteArray) = client.invoke(RepositorySetCredentials(handle, credentials))

    /**
     * Sets, unsets or changes local passwords for accessing the repository or disables the given
     * access mode.
     */
    suspend fun setAccess(read: AccessChange? = null, write: AccessChange? = null) =
        client.invoke(RepositorySetAccess(handle, read, write))

    /**
     * Is Bittorrent DHT enabled?
     *
     * @see setDhtEnabled
     */
    suspend fun isDhtEnabled() = client.invoke(RepositoryIsDhtEnabled(handle)) as Boolean

    /**
     * Enables/disabled Bittorrent DHT (for peer discovery).
     *
     * @see isDhtEnabled
     * @see infoHash
     */
    suspend fun setDhtEnabled(enabled: Boolean) =
        client.invoke(RepositorySetDhtEnabled(handle, enabled))

    /**
     * Is Peer Exchange enabled?
     *
     * @see setPerEnabled
     */
    suspend fun isPexEnabled() = client.invoke(RepositoryIsPexEnabled(handle)) as Boolean

    /**
     * Enables/disables Peer Exchange (for peer discovery).
     *
     * @see isPexEnabled
     */
    suspend fun setPexEnabled(enabled: Boolean) =
        client.invoke(RepositorySetPexEnabled(handle, enabled))

    /**
     * Returns the synchronization progress of this repository
     */
    suspend fun syncProgress() = client.invoke(RepositorySyncProgress(handle)) as Progress

    /**
     * Create mirror of this repository on the cache server(s) that were previously added with
     * [Session.addCacheServer].
     */
    suspend fun mirror() = client.invoke(RepositoryMirror(handle))

    /**
     * Returns the type (file or directory) of an entry at the given path,
     * or `null` if no such entry exists.
     *
     * @param path path of the entry, relative to the repo root.
     */
    suspend fun entryType(path: String): EntryType? {
        val raw = client.invoke(RepositoryEntryType(handle, path)) as Byte?

        if (raw != null) {
            return EntryType.decode(raw)
        } else {
            return null
        }
    }

    /**
     * Moves an entry (file or directory) from `src` to `dst`.
     *
     * @param src path to move the entry from.
     * @param dst path to move the entry to.
     */
    suspend fun moveEntry(src: String, dst: String) =
        client.invoke(RepositoryMoveEntry(handle, src, dst))
}

/**
 * How to change access to a repository.
 *
 * @see [Repository.setAccess]
 */
sealed class AccessChange {
    fun pack(packer: MessagePacker) {
        when (this) {
            is EnableAccess -> {
                packer.packMapHeader(1)
                packer.packString("enable")

                if (password != null) {
                    packer.packString(password)
                } else {
                    packer.packNil()
                }
            }
            is DiableAccess -> {
                packer.packString("disabled")
            }
        }
    }
}

/**
 * Enable read or write access, optionally with local password
 *
 * @see [Repository.setAccess]
 */
class EnableAccess(val password: String?) : AccessChange()

/**
 * Disable access
 *
 * @see [Repository.setAccess]
 */
class DiableAccess() : AccessChange()
