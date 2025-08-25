package org.equalitie.ouisync.kotlin.android

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.database.Cursor
import android.database.MatrixCursor
import android.os.Bundle
import android.os.CancellationSignal
import android.os.Handler
import android.os.ParcelFileDescriptor
import android.os.ProxyFileDescriptorCallback
import android.os.storage.StorageManager
import android.provider.DocumentsContract
import android.provider.DocumentsProvider
import android.system.ErrnoException
import android.system.OsConstants
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
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.equalitie.ouisync.kotlin.client.AccessMode
import org.equalitie.ouisync.kotlin.client.EntryType
import org.equalitie.ouisync.kotlin.client.File
import org.equalitie.ouisync.kotlin.client.OuisyncException
import org.equalitie.ouisync.kotlin.client.Repository
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
                // DocumentsContract.Document.COLUMN_ICON,
            )

        private val ROOT_ID = "default"
        private val ROOT_DOCUMENT_ID = "repos"
    }

    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())

    private val sessionFlow: StateFlow<Session?> by lazy {
        val state = MutableStateFlow<Session?>(null)

        // Receiver that (re)creates the session every time it receives an intent.
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                Log.v(TAG, "receiver.onReceive(action = ${intent.action}, resultCode = $resultCode)")

                val restart = when {
                    intent.action == OuisyncService.ACTION_STARTED ||
                    intent.action == OuisyncService.ACTION_STATUS && resultCode != 0 -> true
                    else -> false
                }

                if (restart) {
                    scope.launch {
                        state.update { session ->
                            session?.close()
                            Session.create(context.getConfigPath())
                        }
                    }
                }
            }
        }

        val context = requireNotNull(context)

        // Trigger the receiver when the service starts
        context.registerReceiver(
            receiver,
            IntentFilter(OuisyncService.ACTION_STARTED),
            Context.RECEIVER_NOT_EXPORTED,
        )

        // Trigger the receiver also if the service has already been started
        context.sendOrderedBroadcast(
            Intent(OuisyncService.ACTION_STATUS),
            null,
            receiver,
            null,
            0,
            null,
            null,
        )

        state.asStateFlow()
    }

    override fun onCreate(): Boolean {
        Log.v(TAG, "onCreate")
        return true
    }

    override fun queryRoots(projection: Array<out String>?): Cursor {
        Log.v(TAG, "queryRoots(${projection?.contentToString()})")

        val result = MatrixCursor(projection ?: DEFAULT_ROOT_PROJECTION)
        val row = result.newRow()

        row.add(DocumentsContract.Root.COLUMN_DOCUMENT_ID, ROOT_DOCUMENT_ID)
        row.add(DocumentsContract.Root.COLUMN_FLAGS, DocumentsContract.Root.FLAG_SUPPORTS_IS_CHILD)
        row.add(DocumentsContract.Root.COLUMN_ICON, R.mipmap.ouisync_provider_root_icon)
        row.add(DocumentsContract.Root.COLUMN_MIME_TYPES, DocumentsContract.Root.MIME_TYPE_ITEM)
        row.add(DocumentsContract.Root.COLUMN_ROOT_ID, ROOT_ID)
        row.add(DocumentsContract.Root.COLUMN_TITLE, "Ouisync")

        return result
    }

    override fun queryChildDocuments(
        parentDocumentId: String?,
        projection: Array<out String>?,
        sortOrder: String?,
    ): Cursor = run {
        Log.v(TAG, "queryChildDocuments($parentDocumentId, ${projection?.contentToString()}, $sortOrder)")

        val locator = Locator.parse(parentDocumentId)
        val result = MatrixCursor(projection ?: DEFAULT_DOCUMENT_PROJECTION)

        if (locator == Locator.ROOT) {
            for (repo in session().listRepositories().values) {
                buildEntryRow(
                    result,
                    repo,
                    EntryType.DIRECTORY,
                    Locator(repo = repo.getShortName(), path = ""),
                )
            }
        } else {
            val repo = session().findRepository(locator.repo)
            val isReadable = when (repo.getAccessMode()) {
                AccessMode.READ, AccessMode.WRITE -> true
                AccessMode.BLIND -> false
            }

            if (isReadable) {
                for (entry in repo.readDirectory(locator.path)) {
                    buildEntryRow(
                        result,
                        repo,
                        entry.entryType,
                        locator.join(entry.name),
                    )
                }
            } else {
                result.setExtras(Bundle().apply {
                    // TODO: localize
                    putString(DocumentsContract.EXTRA_INFO, "This repository is locked")
                })
            }
        }

        result
    }

    override fun queryDocument(documentId: String?, projection: Array<out String>?): Cursor = run {
        Log.v(TAG, "queryDocument($documentId, ${projection?.contentToString()})")

        val locator = Locator.parse(documentId)
        val result = MatrixCursor(projection ?: DEFAULT_DOCUMENT_PROJECTION)

        if (locator == Locator.ROOT) {
            buildRepoListRow(result)
        } else {
            val repo = session().findRepository(locator.repo)
            val entryType = repo.getEntryType(locator.path) ?: throw FileNotFoundException()

            buildEntryRow(result, repo, entryType, locator)
        }

        result
    }

    override fun openDocument(
        documentId: String,
        mode: String,
        signal: CancellationSignal?,
    ): ParcelFileDescriptor {
        Log.v(TAG, "openDocument($documentId, $mode, ..)")

        val context = requireNotNull(context)
        val locator = Locator.parse(documentId)
        val storage = context.getSystemService(Context.STORAGE_SERVICE) as StorageManager
        val handler = Handler(context.mainLooper)

        // TODO: use the cancellation signal

        return storage.openProxyFileDescriptor(
            ParcelFileDescriptor.MODE_READ_ONLY,
            ProxyCallback(locator),
            handler,
        )
    }

    override fun deleteDocument(documentId: String) = run {
        Log.v(TAG, "deleteDocument($documentId)")

        val locator = Locator.parse(documentId)
        require(locator != Locator.ROOT)

        val repo = session().findRepository(locator.repo)

        if (locator.path.isEmpty()) {
            repo.delete()
        } else {
            when (repo.getEntryType(locator.path)) {
                EntryType.FILE -> repo.removeFile(locator.path)
                EntryType.DIRECTORY -> repo.removeDirectory(locator.path, recursive = true)
                null -> throw FileNotFoundException()
            }
        }
    }

    private suspend fun buildEntryRow(cursor: MatrixCursor, repo: Repository, entryType: EntryType, locator: Locator) {
        val row = cursor.newRow()

        row.add(DocumentsContract.Document.COLUMN_DISPLAY_NAME, locator.name)
        row.add(DocumentsContract.Document.COLUMN_DOCUMENT_ID, locator)

        when (entryType) {
            EntryType.FILE -> {
                val file = repo.openFile(locator.path)
                val size = file.getLength()
                val mime = URLConnection.guessContentTypeFromName(locator.name)
                val flags = if (repo.getAccessMode() == AccessMode.WRITE) {
                    DocumentsContract.Document.FLAG_SUPPORTS_DELETE
                } else {
                    0
                }

                row.add(DocumentsContract.Document.COLUMN_SIZE, size)
                row.add(DocumentsContract.Document.COLUMN_MIME_TYPE, mime)
                row.add(DocumentsContract.Document.COLUMN_FLAGS, flags)
            }
            EntryType.DIRECTORY -> {
                val size = if (locator.path.isEmpty()) {
                    repo.getQuota().size.bytes
                } else {
                    null
                }

                val flags = if (locator.path.isEmpty()) {
                    DocumentsContract.Document.FLAG_SUPPORTS_DELETE
                } else if (repo.getAccessMode() == AccessMode.WRITE) {
                    DocumentsContract.Document.FLAG_SUPPORTS_DELETE
                } else {
                    0
                }

                row.add(DocumentsContract.Document.COLUMN_SIZE, size)
                row.add(DocumentsContract.Document.COLUMN_MIME_TYPE, DocumentsContract.Document.MIME_TYPE_DIR)
                row.add(DocumentsContract.Document.COLUMN_FLAGS, flags)
            }
        }

        // TODO: for repos, use custom icon to indicate read/write/blind access
    }

    private fun buildRepoListRow(cursor: MatrixCursor) {
        val row = cursor.newRow()

        // TODO: localize
        row.add(DocumentsContract.Document.COLUMN_DISPLAY_NAME, "Repositories")
        row.add(DocumentsContract.Document.COLUMN_DOCUMENT_ID, ROOT_DOCUMENT_ID)
        row.add(DocumentsContract.Document.COLUMN_FLAGS, 0)
        row.add(DocumentsContract.Document.COLUMN_MIME_TYPE, DocumentsContract.Document.MIME_TYPE_DIR)
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

        val name = if (path.isEmpty()) repo else path.substringAfterLast('/')

        fun join(name: String): Locator = when {
            repo.isEmpty() -> Locator(repo = name, path = "")
            path.isEmpty() -> Locator(repo = repo, path = name)
            else -> Locator(repo = repo, path = "$path/$name")
        }
    }

    private suspend fun session() = sessionFlow.filterNotNull().first()

    private fun <T> run(block: suspend CoroutineScope.() -> T): T = runBlocking {
        scope.block()
    }

    // Callback for proxy file descriptor which wraps a Ouisync file and exposes it as
    // ParcelFileDescriptor.
    private inner class ProxyCallback(
        val locator: Locator,
    ) : ProxyFileDescriptorCallback() {
        private val file: Deferred<File> = scope.async {
            session().findRepository(locator.repo).openFile(locator.path)
        }

        override fun onGetSize() = run("onGetSize") {
            file.await().getLength()
        }

        override fun onRead(
            offset: Long,
            chunkSize: Int,
            outData: ByteArray,
        ) = run("onRead") {
            val chunk = file.await().read(offset, chunkSize.toLong())
            chunk.copyInto(outData)
            chunk.size
        }

        override fun onFsync() = run("onFsync") {
            file.await().flush()
        }

        override fun onRelease() = run("onRelease") {
            file.await().close()
        }

        private fun <T> run(
            name: String,
            block: suspend CoroutineScope.() -> T,
        ): T {
            try {
                return this@OuisyncProvider.run(block)
            } catch (e: Exception) {
                Log.e(
                    TAG,
                    "uncaught exception in ${ProxyCallback::class.simpleName}.$name",
                    e,
                )

                throw ErrnoException(name, e.errno, e)
            }
        }
    }
}

private val Exception.errno: Int
    get() = when (this) {
        is OuisyncException.NotFound -> OsConstants.ENOENT
        is OuisyncException.PermissionDenied -> OsConstants.EPERM
        is OuisyncException.IsDirectory -> OsConstants.EISDIR
        is OuisyncException.NotDirectory -> OsConstants.ENOTDIR
        else -> OsConstants.EIO
    }
