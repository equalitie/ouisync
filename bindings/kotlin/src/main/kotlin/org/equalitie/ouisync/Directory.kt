package org.equalitie.ouisync

import org.msgpack.core.MessageUnpacker
import org.msgpack.value.ValueType

data class DirectoryEntry(val name: String, val entryType: EntryType) {
    companion object {
        internal fun unpack(unpacker: MessageUnpacker) : DirectoryEntry {
            if (unpacker.unpackArrayHeader() != 2) {
                throw InvalidResponse()
            }

            val name = unpacker.unpackString()
            val entryType = EntryType.decode(unpacker.unpackByte())

            return DirectoryEntry(name, entryType)
        }
    }
}

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

    override val size: Int
        get() = entries.size

    override fun isEmpty() = entries.isEmpty()

    override fun iterator() = entries.iterator()

    override operator fun contains(element: DirectoryEntry) =  entries.contains(element)

    override fun containsAll(elements: Collection<DirectoryEntry>) =  entries.containsAll(elements)
}
