package org.equalitie.ouisync.dart

import android.content.BroadcastReceiver
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.CancellationSignal
import android.os.Handler
import android.os.HandlerThread
import android.os.ParcelFileDescriptor
import android.os.ProxyFileDescriptorCallback
import android.os.storage.StorageManager
import android.provider.OpenableColumns
import android.system.ErrnoException
import android.system.OsConstants
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.equalitie.ouisync.kotlin.android.OuisyncService
import org.equalitie.ouisync.kotlin.android.getConfigPath
import org.equalitie.ouisync.kotlin.client.File
import org.equalitie.ouisync.kotlin.client.OuisyncException
import org.equalitie.ouisync.kotlin.client.Session
import org.equalitie.ouisync.kotlin.client.close
import org.equalitie.ouisync.kotlin.client.create
import java.net.URLConnection
import kotlin.collections.joinToString

@Deprecated("use OuisyncProvider")
class PipeProvider : ContentProvider() {
    companion object {
        private val TAG = PipeProvider::class.simpleName

        private const val CHUNK_SIZE = 64000

        private val supportsProxyFileDescriptor: Boolean =
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.O
    }

    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())
    private val session = MutableStateFlow<Session?>(null)

    // Handler for running proxy file descriptor's callbacks
    // TODO: consider using thread pool so we can handle multiple files concurrently.
    private val handler =
        Handler(
            HandlerThread("${javaClass.simpleName} handler thread").apply { start() }.getLooper(),
        )

    private val receiver =
        object : BroadcastReceiver() {
            override fun onReceive(
                context: Context,
                intent: Intent,
            ) {
                scope.launch {
                    session.value?.close()
                    session.value = Session.create(context.getConfigPath())
                }
            }
        }

    override fun onCreate(): Boolean {
        Log.d(TAG, "onCreate")

        val context = requireNotNull(context)
        context.registerReceiver(
            receiver,
            IntentFilter(OuisyncService.ACTION_STARTED),
            Context.RECEIVER_NOT_EXPORTED,
        )
        context.startService(Intent(context, OuisyncService::class.java))

        return true
    }

    override fun query(
        uri: Uri,
        projection: Array<String>?,
        selection: String?,
        selectionArgs: Array<String>?,
        sortOrder: String?,
    ): Cursor? = query(uri, projection, null, null)

    override fun query(
        uri: Uri,
        projection: Array<String>?,
        queryArgs: Bundle?,
        cancellationSignal: CancellationSignal?,
    ): Cursor? {
        val projection = projection ?: arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        val cursor = MatrixCursor(projection, 1)

        val row = cursor.newRow()

        for (col in projection) {
            when (col) {
                OpenableColumns.DISPLAY_NAME -> row.add(uri.lastPathSegment)
                OpenableColumns.SIZE -> row.add(getFileSize(uri))
                else -> row.add(null) // unknown, just add null
            }
        }

        return cursor
    }

    override fun getType(uri: Uri): String? = URLConnection.guessContentTypeFromName(uri.path)

    override fun openFile(
        uri: Uri,
        mode: String,
    ): ParcelFileDescriptor? =
        if (supportsProxyFileDescriptor) {
            openProxyFile(uri)
        } else {
            openPipe(uri)
        }

    override fun insert(
        uri: Uri,
        values: ContentValues?,
    ): Uri? = throw NotImplementedError()

    override fun delete(
        uri: Uri,
        selection: String?,
        selectionArgs: Array<out String>?,
    ): Int = throw NotImplementedError()

    override fun update(
        uri: Uri,
        values: ContentValues?,
        selection: String?,
        selectionArgs: Array<out String>?,
    ): Int = throw NotImplementedError()

    private fun getFileSize(uri: Uri): Long =
        runBlocking(Dispatchers.IO) {
            val file = openRepoFile(uri)

            try {
                file.getLength()
            } finally {
                file.close()
            }
        }

    private fun openProxyFile(uri: Uri): ParcelFileDescriptor? {
        var storage = context!!.getSystemService(Context.STORAGE_SERVICE) as StorageManager

        return storage.openProxyFileDescriptor(
            ParcelFileDescriptor.MODE_READ_ONLY,
            ProxyCallback(uri),
            handler,
        )
    }

    private fun openPipe(uri: Uri): ParcelFileDescriptor? {
        val pipe = ParcelFileDescriptor.createPipe()
        val reader = pipe[0]
        val writer = pipe[1]
        val dstFd = writer!!.detachFd()

        scope.launch {
            val file = openRepoFile(uri)

            try {
                // TODO: copy_to_fd is implemented in the ouisync library but not yet exposed
                // through ousiync-service
                TODO("File.copyToRawFd is not yet implemented")
                // file.copyToFd(dstFd)
            } finally {
                file.close()
            }
        }

        return reader
    }

    private suspend fun openRepoFile(uri: Uri): File {
        val repoName = Uri.decode(uri.pathSegments.first())
        val filePath =
            uri.pathSegments
                .drop(1)
                .map(Uri::decode)
                .joinToString("/")

        return session
            .filterNotNull()
            .first()
            .findRepository(repoName)
            .openFile(filePath)
    }

    inner class ProxyCallback(
        val uri: Uri,
    ) : ProxyFileDescriptorCallback() {
        private val file: Deferred<File> = scope.async { openRepoFile(uri) }

        override fun onGetSize() = run("onGetSize") { file.await().getLength() }

        override fun onRead(
            offset: Long,
            chunkSize: Int,
            outData: ByteArray,
        ) = run("onRead") {
            val chunk = file.await().read(offset, chunkSize.toLong())
            chunk.copyInto(outData)
            chunk.size
        }

        override fun onFsync() = run("onFsync") { file.await().flush() }

        override fun onRelease() = run("onRelease") { file.await().close() }

        private fun <T> run(
            name: String,
            block: suspend CoroutineScope.() -> T,
        ): T =
            try {
                runBlocking(block = block)
            } catch (e: Exception) {
                Log.e(
                    TAG,
                    "uncaught exception in ${ProxyCallback::class.simpleName}.$name ($uri)",
                    e,
                )
                throw ErrnoException(name, e.errno, e)
            }
    }
}

private val Exception.errno: Int
    get() =
        when (this) {
            is OuisyncException.NotFound -> OsConstants.ENOENT
            is OuisyncException.PermissionDenied -> OsConstants.EPERM
            is OuisyncException.IsDirectory -> OsConstants.EISDIR
            is OuisyncException.NotDirectory -> OsConstants.ENOTDIR
            else -> OsConstants.EIO
        }
