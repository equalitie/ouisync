package org.equalitie.ouisync.kotlin.android

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.database.Cursor
import android.database.MatrixCursor
import android.os.CancellationSignal
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.provider.DocumentsProvider
import android.util.Log
import java.io.FileNotFoundException
import java.net.URLConnection
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.equalitie.ouisync.kotlin.client.EntryType
import org.equalitie.ouisync.kotlin.client.Session
import org.equalitie.ouisync.kotlin.client.close
import org.equalitie.ouisync.kotlin.client.create

// Provider that exposes Ouisync repositories to other apps.
class OuisyncProvider : DocumentsProvider() {
    companion object {
        private val TAG = OuisyncProvider::class.simpleName

        val DEFAULT_ROOT_PROJECTION =
            arrayOf(
                DocumentsContract.Root.COLUMN_DOCUMENT_ID,
                DocumentsContract.Root.COLUMN_FLAGS,
                DocumentsContract.Root.COLUMN_ICON,
                DocumentsContract.Root.COLUMN_MIME_TYPES,
                DocumentsContract.Root.COLUMN_ROOT_ID,
                DocumentsContract.Root.COLUMN_SUMMARY,
                DocumentsContract.Root.COLUMN_TITLE,
            )

        val DEFAULT_DOCUMENT_PROJECTION =
            arrayOf(
                DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                DocumentsContract.Document.COLUMN_FLAGS,
                DocumentsContract.Document.COLUMN_MIME_TYPE,
                DocumentsContract.Document.COLUMN_SIZE,
                DocumentsContract.Document.COLUMN_SUMMARY,
                // DocumentsContract.Document.COLUMN_LAST_MODIFIED,
            )

        private val ROOT_ID = "default"
        private val ROOT_DOCUMENT_ID = "repos"
    }

    private val scope = CoroutineScope(Dispatchers.IO)
    private val session: Deferred<Session> = scope.async(start = CoroutineStart.LAZY) {
        Session.create(requireNotNull(context).getConfigPath())
    }

    override fun onCreate(): Boolean {
        Log.d(TAG, "onCreate")
        return true
    }

    override fun queryRoots(projection: Array<out String>?): Cursor {
        Log.d(TAG, "queryRoots(${projection?.contentToString()})")

        val result = MatrixCursor(projection ?: DEFAULT_ROOT_PROJECTION)
        val row = result.newRow()

        row.add(DocumentsContract.Root.COLUMN_DOCUMENT_ID, ROOT_DOCUMENT_ID)
        row.add(DocumentsContract.Root.COLUMN_FLAGS, DocumentsContract.Root.FLAG_SUPPORTS_IS_CHILD)
        row.add(DocumentsContract.Root.COLUMN_ICON, R.mipmap.ouisync_provider_root_icon)
        row.add(DocumentsContract.Root.COLUMN_MIME_TYPES, DocumentsContract.Root.MIME_TYPE_ITEM)
        row.add(DocumentsContract.Root.COLUMN_ROOT_ID, ROOT_ID)

        val numRepos = runBlocking { session.await().listRepositories().size }
        // TODO: localize
        val summary = when (numRepos) {
            0 -> "no repositories"
            1 -> "1 repository"
            else -> "$numRepos repositories"
        }

        row.add(DocumentsContract.Root.COLUMN_SUMMARY, summary)

        row.add(DocumentsContract.Root.COLUMN_TITLE, "Ouisync")

        return result
    }

    override fun queryChildDocuments(
        parentDocumentId: String?,
        projection: Array<out String>?,
        sortOrder: String?,
    ): Cursor {
        Log.d(TAG, "queryChildDocuments($parentDocumentId, ${projection?.contentToString()}, $sortOrder)")

        val locator = Locator.parse(parentDocumentId)
        val result = MatrixCursor(projection ?: DEFAULT_DOCUMENT_PROJECTION)

        if (locator == Locator.ROOT) {
            runBlocking {
                for (repo in session.await().listRepositories().values) {
                    val name = repo.getShortName()
                    val size = repo.getQuota().size

                    val row = result.newRow()

                    row.add(DocumentsContract.Document.COLUMN_DISPLAY_NAME, name)
                    row.add(DocumentsContract.Document.COLUMN_DOCUMENT_ID, Locator(repo = name, path = ""))
                    row.add(DocumentsContract.Document.COLUMN_FLAGS, 0)
                    row.add(DocumentsContract.Document.COLUMN_MIME_TYPE, DocumentsContract.Document.MIME_TYPE_DIR)
                    row.add(DocumentsContract.Document.COLUMN_SIZE, size)
                    row.add(DocumentsContract.Document.COLUMN_SUMMARY, "TODO: repo summary")
                }
            }
        } else {
            runBlocking {
                val repo = session.await().findRepository(locator.repo)

                for (entry in repo.readDirectory(locator.path)) {
                    val entryLocator = locator.join(entry.name)

                    val row = result.newRow()

                    row.add(DocumentsContract.Document.COLUMN_DISPLAY_NAME, entry.name)
                    row.add(DocumentsContract.Document.COLUMN_DOCUMENT_ID, entryLocator)
                    row.add(DocumentsContract.Document.COLUMN_FLAGS, 0)

                    val mime = when (entry.entryType) {
                        EntryType.FILE -> URLConnection.guessContentTypeFromName(entry.name)
                        EntryType.DIRECTORY -> DocumentsContract.Document.MIME_TYPE_DIR
                    }

                    val size = when (entry.entryType) {
                        EntryType.FILE -> repo.openFile(entryLocator.path).getLength()
                        EntryType.DIRECTORY -> null
                    }

                    row.add(DocumentsContract.Document.COLUMN_MIME_TYPE, mime)
                    row.add(DocumentsContract.Document.COLUMN_SIZE, size)
                    row.add(DocumentsContract.Document.COLUMN_SUMMARY, "TODO: entry summary")
                }
            }
        }

        return result
    }

    override fun queryDocument(documentId: String?, projection: Array<out String>?): Cursor {
        Log.d(TAG, "queryDocument($documentId, ${projection?.contentToString()})")

        val locator = Locator.parse(documentId)
        val result = MatrixCursor(projection ?: DEFAULT_DOCUMENT_PROJECTION)
        val row = result.newRow()

        if (locator == Locator.ROOT) {
            // TODO: localize
            row.add(DocumentsContract.Document.COLUMN_DISPLAY_NAME, "Repositories")
            row.add(DocumentsContract.Document.COLUMN_DOCUMENT_ID, documentId)
            row.add(DocumentsContract.Document.COLUMN_FLAGS, 0)
            row.add(DocumentsContract.Document.COLUMN_MIME_TYPE, DocumentsContract.Document.MIME_TYPE_DIR)
        } else {
            row.add(DocumentsContract.Document.COLUMN_DISPLAY_NAME, locator.name)
            row.add(DocumentsContract.Document.COLUMN_DOCUMENT_ID, documentId)
            row.add(DocumentsContract.Document.COLUMN_FLAGS, 0)

            val type = runBlocking {
                session
                    .await()
                    .findRepository(locator.repo)
                    .getEntryType(locator.path)
            }

            val mime = when (type) {
                EntryType.FILE -> URLConnection.guessContentTypeFromName(locator.path)
                EntryType.DIRECTORY -> DocumentsContract.Document.MIME_TYPE_DIR
                null -> throw FileNotFoundException()
            }

            row.add(DocumentsContract.Document.COLUMN_MIME_TYPE, mime)
        }

        return result
    }

    override fun openDocument(
        documentId: String,
        mode: String,
        signal: CancellationSignal?,
    ): ParcelFileDescriptor {
        Log.d(TAG, "openDocument($documentId, $mode, ..)")

        throw NotImplementedError()
    }



    private data class Locator(val repo: String, val path: String) {
        companion object {
            fun parse(documentId: String?): Locator {
                if (documentId == null || documentId == ROOT_DOCUMENT_ID) {
                    return ROOT
                }

                val index = documentId.indexOf('/')
                require(index >= 0) { "invalid document id" }

                return Locator(
                    repo = documentId.substring(0, index),
                    path = documentId.substring(index + 1),
                )
            }

            val ROOT = Locator("", "")
        }

        override fun toString() = if (repo.isEmpty()) ROOT_DOCUMENT_ID else "$repo/$path"

        val name: String = path.substringAfterLast('/')

        fun join(name: String): Locator = when {
            repo.isEmpty() -> Locator(repo = name, path = "")
            path.isEmpty() -> Locator(repo = repo, path = name)
            else -> Locator(repo = repo, path = "$path/$name")
        }
    }
}
