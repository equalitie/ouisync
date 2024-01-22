package org.equalitie.ouisync

import org.msgpack.core.MessageUnpacker

/**
 * A directory entry
 *
 * @property name      name of the entry.
 * @property entryType type of the entry (i.e., file or directory).
 */
data class DirectoryEntry(val name: String, val entryType: EntryType) {
    companion object {
        internal fun unpack(unpacker: MessageUnpacker): DirectoryEntry {
            if (unpacker.unpackArrayHeader() != 2) {
                throw InvalidResponse()
            }

            val name = unpacker.unpackString()
            val entryType = EntryType.decode(unpacker.unpackByte())

            return DirectoryEntry(name, entryType)
        }
    }
}

/**
 * A read-only snapshot of a directory stored in a Ouisync repository
 */
class Directory private constructor(private val entries: List<DirectoryEntry>) : Collection<DirectoryEntry> {
    companion object {
        /**
         * Creates a new directory at the given path in the repository.
         */
        suspend fun create(repo: Repository, path: String) {
            repo.client.invoke(DirectoryCreate(repo.handle, path))
        }

        /**
         * Opens an existing directory at the given path in the repository.
         */
        suspend fun open(repo: Repository, path: String): Directory {
            return repo.client.invoke(DirectoryOpen(repo.handle, path)) as Directory
        }

        /**
         * Removes file at the given path from the repository.
         */
        suspend fun remove(repo: Repository, path: String, recursive: Boolean = false) {
            repo.client.invoke(DirectoryRemove(repo.handle, path, recursive))
        }

        internal fun unpack(unpacker: MessageUnpacker): Directory {
            val count = unpacker.unpackArrayHeader()
            val entries = 0.rangeUntil(count).map { DirectoryEntry.unpack(unpacker) }

            return Directory(entries)
        }
    }

    /**
     * Number of entries in this directory.
     */
    override val size: Int
        get() = entries.size

    /**
     * Is this directory empty?
     */
    override fun isEmpty() = entries.isEmpty()

    /**
     * Returns an iterator over the entries of this directory.
     */
    override fun iterator() = entries.iterator()

    /**
     * Checks if the specified entry is contained in this directory.
     */
    override operator fun contains(element: DirectoryEntry) = entries.contains(element)

    /**
     * Checks if all entries in the specified collection are contained in this directory.
     */
    override fun containsAll(elements: Collection<DirectoryEntry>) = entries.containsAll(elements)
}
