package org.equalitie.ouisync.kotlin.android

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ProviderInfo
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.Bundle
import android.os.CancellationSignal
import android.os.Handler
import android.os.HandlerThread
import android.os.ParcelFileDescriptor
import android.os.ProxyFileDescriptorCallback
import android.os.storage.StorageManager
import android.provider.DocumentsContract
import android.provider.DocumentsProvider
import android.system.ErrnoException
import android.system.OsConstants
import android.util.Log
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.equalitie.ouisync.session.AccessMode
import org.equalitie.ouisync.session.EntryType
import org.equalitie.ouisync.session.File
import org.equalitie.ouisync.session.OuisyncException
import org.equalitie.ouisync.session.Repository
import org.equalitie.ouisync.session.Session
import org.equalitie.ouisync.session.close
import org.equalitie.ouisync.session.create
import org.equalitie.ouisync.session.subscribe
import java.io.FileNotFoundException
import java.net.URLConnection

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
    }

    private val scope = CoroutineScope(SupervisorJob())

    // StateFlow that emits new session every time OuisyncService is (re)started.
    private val sessionFlow: StateFlow<Session?> by lazy {
        val state = MutableStateFlow<Session?>(null)

        // Receiver that (re)creates the session every time it receives an intent.
        val receiver =
            object : BroadcastReceiver() {
                override fun onReceive(context: Context, intent: Intent) {
                    val restart =
                        when {
                            intent.action == OuisyncService.ACTION_STARTED ||
                                intent.action == OuisyncService.ACTION_STATUS &&
                                resultCode != 0 -> true
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
            Intent(OuisyncService.ACTION_STATUS).setPackage(context.getPackageName()),
            null,
            receiver,
            null,
            0, // The initial result code. If the service is running, it sets this to 1.
            null,
            null,
        )

        state.asStateFlow()
    }

    // Handler for running the proxy file descriptor callbacks.
    private val handler =
        Handler(HandlerThread("${this::class.simpleName} proxy").apply { start() }.looper)

    private var authority: String? = null

    private val subscriptions = SubscriptionMap()

    override fun onCreate(): Boolean {
        Log.v(TAG, "onCreate")
        return true
    }

    // Override this method to get the authority of this provider which is needed to construct
    // notification uris.
    override fun attachInfo(context: Context, info: ProviderInfo) {
        super.attachInfo(context, info)
        authority = info.authority
    }

    override fun isChildDocument(parentDocumentId: String, documentId: String): Boolean {
        Log.v(TAG, "isChildDocument($parentDocumentId, $documentId)")
        return Locator.parse(documentId).isChildOf(Locator.parse(parentDocumentId))
    }

    override fun getDocumentType(documentId: String): String {
        Log.v(TAG, "getDocumentMimeType($documentId)")

        return getFileMimeType(Locator.parse(documentId).name)
    }

    override fun queryRoots(projection: Array<String>?): Cursor {
        Log.v(TAG, "queryRoots(${projection?.contentToString()})")

        val context = requireNotNull(context)
        val uri = DocumentsContract.buildRootsUri(requireNotNull(authority))

        val result = MatrixCursor(projection ?: DEFAULT_ROOT_PROJECTION)
        result.setNotificationUri(context.contentResolver, uri)

        val row = result.newRow()
        row.add(DocumentsContract.Root.COLUMN_DOCUMENT_ID, Locator.ROOT_DOCUMENT_ID)
        row.add(
            DocumentsContract.Root.COLUMN_FLAGS,
            DocumentsContract.Root.FLAG_SUPPORTS_IS_CHILD or
                DocumentsContract.Root.FLAG_SUPPORTS_CREATE,
        )
        row.add(DocumentsContract.Root.COLUMN_ICON, R.mipmap.ouisync_provider_root_icon)
        row.add(DocumentsContract.Root.COLUMN_MIME_TYPES, DocumentsContract.Root.MIME_TYPE_ITEM)
        row.add(DocumentsContract.Root.COLUMN_ROOT_ID, ROOT_ID)
        row.add(DocumentsContract.Root.COLUMN_TITLE, context.getString(R.string.ouisync_provider_name))

        return result
    }

    override fun queryChildDocuments(
        parentDocumentId: String?,
        projection: Array<String>?,
        sortOrder: String?,
    ): Cursor = run {
        Log.v(
            TAG,
            "queryChildDocuments($parentDocumentId, ${projection?.contentToString()}, $sortOrder)",
        )

        val locator = Locator.parse(parentDocumentId)
        val context = requireNotNull(context)
        val uri = DocumentsContract.buildChildDocumentsUri(requireNotNull(authority), parentDocumentId)

        if (locator.isRoot()) {
            // TODO: notify on repo list changes
            val result = MatrixCursor(projection ?: DEFAULT_DOCUMENT_PROJECTION)
            result.setNotificationUri(context.contentResolver, uri)

            for (repo in session().listRepositories().values) {
                buildEntryRow(
                    result,
                    repo,
                    EntryType.DIRECTORY,
                    Locator(repo = repo.getShortName(), path = ""),
                )
            }

            result
        } else {
            val repo = session().findRepository(locator.repo)
            val isReadable =
                when (repo.getAccessMode()) {
                    AccessMode.READ,
                    AccessMode.WRITE,
                    -> true
                    AccessMode.BLIND -> false
                }

            subscriptions.insert(repo, uri)

            val result =
                object : MatrixCursor(projection ?: DEFAULT_DOCUMENT_PROJECTION) {
                    override fun close() {
                        subscriptions.remove(repo, uri)
                        super.close()
                    }
                }

            result.setNotificationUri(context.contentResolver, uri)

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
                result.setExtras(
                    Bundle().apply {
                        putString(
                            DocumentsContract.EXTRA_INFO,
                            context.getString(R.string.ouisync_repository_is_locked),
                        )
                    },
                )
            }

            result
        }
    }

    override fun queryDocument(documentId: String?, projection: Array<String>?): Cursor = run {
        Log.v(TAG, "queryDocument($documentId, ${projection?.contentToString()})")

        val locator = Locator.parse(documentId)
        val context = requireNotNull(context)
        val uri = DocumentsContract.buildDocumentUri(requireNotNull(authority), documentId)

        val result = MatrixCursor(projection ?: DEFAULT_DOCUMENT_PROJECTION)
        result.setNotificationUri(context.contentResolver, uri)

        if (locator.isRoot()) {
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
        val mode = ParcelFileDescriptor.parseMode(mode)

        return storage.openProxyFileDescriptor(
            mode,
            ProxyCallback(locator, signal),
            handler,
        )
    }

    override fun createDocument(
        parentDocumentId: String,
        mimeType: String,
        displayName: String,
    ): String = run {
        Log.v(TAG, "createDocument($parentDocumentId, $mimeType, $displayName)")

        val parentLocator = Locator.parse(parentDocumentId)

        if (parentLocator.isRoot()) {
            throw UnsupportedOperationException("Create repository not supported")
        }

        val locator = parentLocator.join(displayName)
        val repo = session().findRepository(locator.repo)

        if (mimeType == DocumentsContract.Document.MIME_TYPE_DIR) {
            repo.createDirectory(locator.path)
        } else {
            repo.createFile(locator.path).close()
        }

        notifyChildDocumentsChange(parentLocator.toString())

        locator.toString()
    }

    override fun deleteDocument(documentId: String) = run {
        Log.v(TAG, "deleteDocument($documentId)")

        val locator = Locator.parse(documentId)
        require(!locator.isRoot())

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

        notifyChildDocumentsChange(locator.parent.toString())
    }

    override fun renameDocument(documentId: String, displayName: String): String? = run {
        Log.v(TAG, "renameDocument($documentId, $displayName)")

        val srcLocator = Locator.parse(documentId)

        if (srcLocator.path.isEmpty()) {
            throw UnsupportedOperationException("Rename repository not supported")
        }

        val dstLocator = srcLocator.parent.join(displayName)

        session().findRepository(srcLocator.repo).moveEntry(srcLocator.path, dstLocator.path)

        notifyChildDocumentsChange(dstLocator.parent.toString())

        dstLocator.toString()
    }

    override fun moveDocument(
        sourceDocumentId: String,
        sourceParentDocumentId: String,
        targetParentDocumentId: String,
    ): String = run {
        Log.v(TAG, "moveDocument($sourceDocumentId, $sourceParentDocumentId, $targetParentDocumentId)")

        val srcLocator = Locator.parse(sourceDocumentId)

        if (srcLocator.path.isEmpty()) {
            throw UnsupportedOperationException("Move repository is not supported")
        }

        val dstLocator = Locator.parse(targetParentDocumentId).join(srcLocator.name)

        if (srcLocator.repo != dstLocator.repo) {
            throw UnsupportedOperationException("Move between repositories not supported")
        }

        session().findRepository(srcLocator.repo).moveEntry(srcLocator.path, dstLocator.path)

        revokeDocumentPermission(sourceDocumentId)

        notifyChildDocumentsChange(sourceParentDocumentId, targetParentDocumentId)

        dstLocator.toString()
    }

    private suspend fun buildEntryRow(
        cursor: MatrixCursor,
        repo: Repository,
        entryType: EntryType,
        locator: Locator,
    ) {
        val row = cursor.newRow()

        row.add(DocumentsContract.Document.COLUMN_DISPLAY_NAME, locator.name)
        row.add(DocumentsContract.Document.COLUMN_DOCUMENT_ID, locator)

        when (entryType) {
            EntryType.FILE -> {
                val mime = getFileMimeType(locator.name)
                val flags =
                    when (repo.getAccessMode()) {
                        AccessMode.WRITE -> {
                            DocumentsContract.Document.FLAG_SUPPORTS_DELETE or
                                DocumentsContract.Document.FLAG_SUPPORTS_MOVE or
                                DocumentsContract.Document.FLAG_SUPPORTS_RENAME
                        }
                        AccessMode.READ -> 0
                        AccessMode.BLIND -> 0
                    }

                row.add(DocumentsContract.Document.COLUMN_MIME_TYPE, mime)

                try {
                    val file = repo.openFile(locator.path)
                    val size = file.getLength()
                    val progress = file.getProgress()

                    if (progress < size) {
                        row.add(DocumentsContract.Document.COLUMN_SIZE, progress)
                        row.add(
                            DocumentsContract.Document.COLUMN_FLAGS,
                            flags or DocumentsContract.Document.FLAG_PARTIAL,
                        )
                    } else {
                        row.add(DocumentsContract.Document.COLUMN_SIZE, size)
                        row.add(DocumentsContract.Document.COLUMN_FLAGS, flags)
                    }
                } catch (e: OuisyncException.StoreError) {
                    // `StoreError` is typically caused by some blocks not being loaded yet.
                    row.add(DocumentsContract.Document.COLUMN_SIZE, null)
                    row.add(
                        DocumentsContract.Document.COLUMN_FLAGS,
                        flags or DocumentsContract.Document.FLAG_PARTIAL,
                    )
                }
            }
            EntryType.DIRECTORY -> {
                val size =
                    if (locator.path.isEmpty()) {
                        repo.getQuota().size.bytes
                    } else {
                        null
                    }

                val flags =
                    when (repo.getAccessMode()) {
                        AccessMode.WRITE -> {
                            DocumentsContract.Document.FLAG_DIR_SUPPORTS_CREATE or
                                // TODO: Deleting and moving/renaming repos disabled until repo list change
                                // notifications are implemented.
                                if (locator.path.isEmpty()) {
                                    0
                                } else {
                                    DocumentsContract.Document.FLAG_SUPPORTS_DELETE or
                                        DocumentsContract.Document.FLAG_SUPPORTS_MOVE or
                                        DocumentsContract.Document.FLAG_SUPPORTS_RENAME
                                }
                        }
                        AccessMode.READ -> 0
                        AccessMode.BLIND -> 0
                    }

                row.add(DocumentsContract.Document.COLUMN_SIZE, size)
                row.add(
                    DocumentsContract.Document.COLUMN_MIME_TYPE,
                    DocumentsContract.Document.MIME_TYPE_DIR,
                )
                row.add(DocumentsContract.Document.COLUMN_FLAGS, flags)
            }
        }

        // TODO: for repos, use custom icon to indicate read/write/blind access
    }

    private fun buildRepoListRow(cursor: MatrixCursor) {
        val context = requireNotNull(context)
        val row = cursor.newRow()

        row.add(
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            context.getString(R.string.ouisync_repositories),
        )
        row.add(DocumentsContract.Document.COLUMN_DOCUMENT_ID, Locator.ROOT_DOCUMENT_ID)
        row.add(DocumentsContract.Document.COLUMN_FLAGS, 0)
        row.add(DocumentsContract.Document.COLUMN_MIME_TYPE, DocumentsContract.Document.MIME_TYPE_DIR)
    }

    private fun getFileMimeType(name: String): String = URLConnection.guessContentTypeFromName(name) ?: "application/octet-stream"

    private fun notifyChildDocumentsChange(vararg parentDocumentIds: String) {
        val context = requireNotNull(context)
        val authority = requireNotNull(authority)

        for (parentDocumentId in parentDocumentIds) {
            val uri = DocumentsContract.buildChildDocumentsUri(authority, parentDocumentId)
            context.contentResolver.notifyChange(uri, null)
        }
    }

    private suspend fun session() = sessionFlow.filterNotNull().first()

    private fun <T> run(block: suspend CoroutineScope.() -> T): T = runBlocking(scope.coroutineContext, block)

    // Callback for proxy file descriptor which wraps a Ouisync file and exposes it as
    // ParcelFileDescriptor.
    private inner class ProxyCallback(
        val locator: Locator,
        val signal: CancellationSignal?,
    ) : ProxyFileDescriptorCallback() {
        private val scope =
            CoroutineScope(SupervisorJob()).also { scope ->
                signal?.setOnCancelListener { scope.cancel() }

                if (signal?.isCanceled() ?: false) {
                    scope.cancel()
                }
            }

        private val file: Deferred<File> =
            scope.async(start = CoroutineStart.LAZY) {
                session().findRepository(locator.repo).openFile(locator.path)
            }

        override fun onGetSize() = run("onGetSize") { file.await().getLength() }

        override fun onRead(offset: Long, size: Int, data: ByteArray) = run("onRead") {
            val chunk = file.await().read(offset, size.toLong())
            chunk.copyInto(data)
            chunk.size
        }

        override fun onWrite(offset: Long, size: Int, data: ByteArray) = run("onWrite") {
            file.await().write(offset, data.copyOfRange(0, size))
            size
        }

        override fun onFsync() = run("onFsync") { file.await().flush() }

        override fun onRelease() = run("onRelease") { file.await().close() }

        private fun <T> run(
            name: String,
            block: suspend CoroutineScope.() -> T,
        ): T {
            try {
                return runBlocking(scope.coroutineContext, block)
            } catch (e: Exception) {
                Log.e(
                    TAG,
                    "uncaught exception in ${ProxyCallback::class.simpleName}.$name with $locator",
                    e,
                )

                throw ErrnoException(name, e.errno, e)
            }
        }
    }

    // Container for repository subscriptions to preserve them across successive queries.
    private inner class SubscriptionMap {
        val entries: HashMap<SubscriptionKey, SubscriptionData> = HashMap()

        @Synchronized
        fun insert(repo: Repository, uri: Uri) {
            val key = SubscriptionKey(repo, uri)

            entries
                .getOrPut(key) {
                    val contentResolver = requireNotNull(context).contentResolver
                    val job =
                        scope.launch {
                            Log.v(TAG, "subscribe to $uri")
                            repo.subscribe().collect { contentResolver.notifyChange(uri, null) }
                        }

                    SubscriptionData(job)
                }
                .refcount += 1
        }

        @Synchronized
        fun remove(repo: Repository, uri: Uri) {
            val key = SubscriptionKey(repo, uri)
            val data = entries.get(key)
            if (data == null) return

            data.refcount -= 1

            if (data.refcount <= 0) {
                Log.v(TAG, "unsubscribe from $uri")
                data.job.cancel()
                entries.remove(key)
            }
        }
    }

    private data class SubscriptionKey(val repo: Repository, val uri: Uri)

    private class SubscriptionData(val job: Job, var refcount: Int = 0)
}

private val Exception.errno: Int
    get() =
        when (this) {
            is OuisyncException.NotFound -> OsConstants.ENOENT
            is OuisyncException.PermissionDenied -> OsConstants.EPERM
            is OuisyncException.IsDirectory -> OsConstants.EISDIR
            is OuisyncException.NotDirectory -> OsConstants.ENOTDIR
            is CancellationException -> OsConstants.ECANCELED
            else -> OsConstants.EIO
        }
