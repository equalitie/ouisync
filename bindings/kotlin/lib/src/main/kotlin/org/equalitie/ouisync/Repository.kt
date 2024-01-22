package org.equalitie.ouisync

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

        /**
         * Reopen a repo using a *reopen token*.
         *
         * This is useful when one wants to move the repo to a different location while the repo is
         * currently open without requiring the user to input the local password again.
         * To do that:
         *
         * 1. Obtain the reopen token with [createReopenToken].
         * 2. [Close][close] the repo.
         * 3. Move the repo file.
         * 4. Reopen the repo with [reopen], passing it the reopen token from step 1.
         *
         * @see createReopenToken
         */
        suspend fun reopen(session: Session, path: String, token: ByteArray): Repository {
            val client = session.client
            val handle = client.invoke(RepositoryReopen(path, token)) as Long

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
     * Returns the access mode (*blind*, *read* or *write*) the repo is opened in.
     */
    suspend fun accessMode(): AccessMode {
        val raw = client.invoke(RepositoryAccessMode(handle)) as Byte
        return AccessMode.decode(raw)
    }

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
     * Creates a token for reopening the repo after it's been renamed/moved.
     *
     * @see reopen
     */
    suspend fun createReopenToken() =
        client.invoke(RepositoryCreateReopenToken(handle)) as ByteArray

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
     * Changes the local read password.
     *
     * The repo must be either opened in at least read mode or the `shareToken` must be set to a
     * token with at least read access, otherwise [Error] with [ErrorCode.PERMISSION_DENIED] is
     * thrown.
     *
     * @param password   new password to set. If null, local read password is removed (the repo
     *                   becomes readable without a password).
     * @param shareToken share token to use in case the repo is currently not opened in at least
     *                   read mode. Can be used to escalate access mode from blind to read.
     */
    suspend fun setReadAccess(password: String?, shareToken: ShareToken? = null) =
        client.invoke(RepositorySetReadAccess(handle, password, shareToken?.toString()))


    /**
     * Changes the local read and write password.
     *
     * The repo must be either opened in write mode or the `shareToken` must be set to a token with
     * write mode, otherwise [Error] with [ErrorCode.PERMISSION_DENIED] is thrown.
     *
     * To set different read and write passwords, call first this function and then [setReadAccess].
     *
     * @param oldPassword previous local write password. This is optional and only needed if one
     *                    wants to preserve the *writer id* of this replica. If null, this
     *                    repository effectively becomes a different replica from the one it was
     *                    before this function was called. This currently has no user observable
     *                    effect apart from slight performance impact.
     * @param newPassword new local read and write password to set. If null, password access is
     *                    removed.
     * @param shareToken  share token to use in case the repo is currently not opened in write mode.
     *                    Can be used to escalate access mode from blind or read to write.
     */
    suspend fun setReadAndWriteAccess(oldPassword: String?, newPassword: String?, shareToken: ShareToken? = null) =
        client.invoke(RepositorySetReadAndWriteAccess(handle, oldPassword, newPassword, shareToken?.toString()))

    /**
     * Removes read access to the repository using the local read password.
     */
    suspend fun removeReadKey() = client.invoke(RepositoryRemoveReadKey(handle))

    /**
     * Removes write access to the repository using the local write password.
     */
    suspend fun removeWriteKey() = client.invoke(RepositoryRemoveWriteKey(handle))

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
     * @see [infoHash]
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
