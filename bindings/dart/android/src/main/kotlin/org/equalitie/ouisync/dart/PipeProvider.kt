package org.equalitie.ouisync.dart

import android.content.ComponentName
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.CancellationSignal
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.ParcelFileDescriptor
import android.os.ProxyFileDescriptorCallback
import android.os.storage.StorageManager
import android.provider.OpenableColumns
import android.util.Log
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.equalitie.ouisync.kotlin.client.File
import org.equalitie.ouisync.kotlin.client.Session
import org.equalitie.ouisync.kotlin.client.create
import java.net.URLConnection
import kotlin.collections.joinToString
import kotlin.coroutines.resume
import kotlin.coroutines.suspendCoroutine

class PipeProvider : ContentProvider() {
    companion object {
        private const val CHUNK_SIZE = 64000

        private val supportsProxyFileDescriptor: Boolean =
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.O
    }

    private val supervisorJob = SupervisorJob()
    private val scope = CoroutineScope(Dispatchers.IO + supervisorJob)

    private val session = CompletableDeferred<Session>()

    // Handler for running proxy file descriptor's callbacks
    private val handler =
        Handler(
            HandlerThread("${javaClass.simpleName} handler thread")
                .apply {
                    setUncaughtExceptionHandler { thread, e ->
                        Log.e(TAG, "uncaught exception in ${thread.name}", e)
                    }
                    start()
                }.getLooper(),
        )

    override fun onCreate(): Boolean {
        val context = requireNotNull(context)

        scope.launch {
            // Bind to OusyncService and wait until the ousiync service has been started.
            suspendCoroutine<Unit> { cont ->
                val intent = Intent(context, OuisyncService::class.java)
                val connection =
                    object : ServiceConnection {
                        override fun onServiceConnected(
                            name: ComponentName,
                            binder: IBinder,
                        ) = (binder as OuisyncService.LocalBinder).onStart { cont.resume(Unit) }

                        override fun onServiceDisconnected(name: ComponentName) = Unit
                    }

                context.bindService(intent, connection, 0)
            }

            // Create ousync Session which should connect to the server we just started above.
            val configPath = context.loadConfigPath()
            session.complete(Session.create(configPath))
        }

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
        runBlocking {
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
                TODO("File.copyToRawFd is not yet implemented")
                // file.copyToFd(dstFd)
            } finally {
                file.close()
            }
        }

        return reader
    }

    private suspend fun openRepoFile(uri: Uri): File {
        val repoName = uri.pathSegments.first()
        val filePath = uri.pathSegments.drop(1).joinToString("/")

        return session.await().findRepository(repoName).openFile(filePath)
    }

    inner class ProxyCallback(
        val uri: Uri,
    ) : ProxyFileDescriptorCallback() {
        private val file: Deferred<File> by lazy { scope.async { openRepoFile(uri) } }

        override fun onGetSize() = runBlocking { file.await().getLength() }

        override fun onRead(
            offset: Long,
            chunkSize: Int,
            outData: ByteArray,
        ) = runBlocking {
            val chunk = file.await().read(offset, chunkSize.toLong())
            chunk.copyInto(outData)
            chunk.size
        }

        override fun onFsync() = runBlocking { file.await().flush() }

        override fun onRelease() = runBlocking { file.await().close() }
    }
}
