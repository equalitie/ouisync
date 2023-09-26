package org.equalitie.ouisync

class File private constructor(private val handle: Long, private val client: Client) {
    companion object {
        /**
         * Creates a new file at the given path in the repository.
         */
        suspend fun create(repo: Repository, path: String): File {
            val handle = repo.client.invoke(FileCreate(repo.handle, path)) as Long
            return File(handle, repo.client)
        }

        /**
         * Opens an existing file at the given path in the repository.
         */
        suspend fun open(repo: Repository, path: String): File {
            val handle = repo.client.invoke(FileOpen(repo.handle, path)) as Long
            return File(handle, repo.client)
        }

        /**
         * Removes file at the given path from the repository.
         */
        suspend fun remove(repo: Repository, path: String) {
            repo.client.invoke(FileRemove(repo.handle, path))
        }
    }

    suspend fun close() = client.invoke(FileClose(handle))

    suspend fun flush() = client.invoke(FileFlush(handle))

    /**
     * Length of this file in bytes
     */
    suspend fun length() = client.invoke(FileLen(handle)) as Long

    /**
     * Read the given amount of bytes from this file, starting at the given offset.
     */
    suspend fun read(offset: Long, length: Long) =
        client.invoke(FileRead(handle, offset, length)) as ByteArray

    /**
     * Write the content of the array to the file at the given offset.
     */
    suspend fun write(offset: Long, array: ByteArray) =
        client.invoke(FileWrite(handle, offset, array))

    /**
     * Truncate the file to the given length.
     */
    suspend fun truncate(length: Long) = client.invoke(FileTruncate(handle, length))

    /**
     * Sync progress of this file, that is, what part of this file (in bytes) is available locally.
     */
    suspend fun progress() = client.invoke(FileProgress(handle)) as Long
}
