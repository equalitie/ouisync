package org.equalitie.ouisync.lib

import android.content.Context
import android.content.res.AssetFileDescriptor
import android.net.Uri
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.os.storage.StorageManager
import android.system.ErrnoException
import android.system.OsConstants
import android.util.Log
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.MethodChannel.Result
import java.io.FileNotFoundException
import java.io.IOException
import java.util.concurrent.CompletableFuture

class PipeProvider: AbstractFileProvider() {
    companion object {
        private const val CHUNK_SIZE = 64000
        internal val TAG = PipeProvider::class.java.simpleName

        private val supportsProxyFileDescriptor: Boolean
            get() = android.os.Build.VERSION.SDK_INT >= 26
    }

    private lateinit var handler: Handler

    override fun onCreate(): Boolean {
        Log.d(TAG, "onCreate")

        val thread = HandlerThread("${javaClass.simpleName} worker thread")
        thread.start();

        handler = Handler(thread.getLooper())

        return true
    }

    // TODO: Handle `mode`
    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor? {
        Log.d(TAG, "Opening file '$uri' in mode '$mode'")

        val path = getPathFromUri(uri);

        if (supportsProxyFileDescriptor) {
            var size = super.getDataLength(uri);

            if (size == AssetFileDescriptor.UNKNOWN_LENGTH) {
                Log.d(TAG, "Using pipe because size is unknown");
                return openPipe(path);
            }

            Log.d(TAG, "Using proxy file");
            return openProxyFile(path, size);
        } else {
            Log.d(TAG, "Using pipe because proxy file is not supported");
            return openPipe(path);
        }
    }

    private fun openProxyFile(path: String, size: Long): ParcelFileDescriptor? {
        var storage = context!!.getSystemService(Context.STORAGE_SERVICE) as StorageManager;

        // https://developer.android.google.cn/reference/android/os/storage/StorageManager
        return storage.openProxyFileDescriptor(
            ParcelFileDescriptor.MODE_READ_ONLY,
            ProxyCallbacks(path, size),
            handler
        )
    }

    private fun openPipe(path: String): ParcelFileDescriptor? {
        val pipe = ParcelFileDescriptor.createPipe()
        var reader = pipe[0]
        var writer = pipe[1]
        var dstFd = writer!!.detachFd();

        runInMainThread {
            copyFileToRawFd(path, dstFd, object: MethodChannel.Result {
                override fun success(a: Any?) {
                    writer.close()
                }

                override fun error(code: String, message: String?, details: Any?) {
                    Log.e(TAG, channelMethodErrorMessage(code, message, details))
                    writer.close()
                }

                override fun notImplemented() {
                    writer.close()
                }
            })
        }

        return reader
    }

    private fun getPathFromUri(uri: Uri): String {
        val segments = uri.pathSegments
        var index = 0
        var path = ""

        for (segment in segments) {
            if (index > 0) {
                path += "/$segment"
            }
            index++
        }

        return path
    }

    internal class ProxyCallbacks(
        private val path: String,
        private val size: Long
    ) : android.os.ProxyFileDescriptorCallback() {
        private var id: Int? = null

        override fun onGetSize() = size

        override fun onRead(offset: Long, chunkSize: Int, outData: ByteArray): Int {
            var id = this.id

            if (id == null) {
                id = openFile(path) ?: throw ErrnoException("openFile", OsConstants.ENOENT)
                this.id = id
            }

            val chunk = readFile(id, chunkSize, offset)

            if (chunk != null) {
                chunk.copyInto(outData)
                return chunk.size
            } else {
                return 0
            }
        }

        override fun onRelease() {
            val id = this.id

            if (id != null) {
                closeFile(id)
            }
        }
    }
}

private fun openFile(path: String): Int? {
    val arguments = hashMapOf("path" to path)
    return invokeBlocking("openFile", arguments)
}

private fun closeFile(id: Int) {
    val arguments = hashMapOf("id" to id)
    invokeBlocking<Unit>("closeFile", arguments)
}

private fun readFile(id: Int, chunkSize: Int, offset: Long): ByteArray? {
    val arguments = hashMapOf("id" to id, "chunkSize" to chunkSize, "offset" to offset)
    return invokeBlocking("readFile", arguments)
}

private fun copyFileToRawFd(srcPath: String, dstFd: Int, result: MethodChannel.Result) {
    val arguments = hashMapOf("srcPath" to srcPath, "dstFd" to dstFd)
    val channel = OuisyncPlugin.sharedChannel

    if (channel != null) {
        channel.invokeMethod("copyFileToRawFd", arguments, result)
    } else {
        result.notImplemented()
    }
}

private fun <T> invokeBlocking(method: String, arguments: Any?): T? {
    val channel = OuisyncPlugin.sharedChannel

    if (channel == null) {
        Log.w(PipeProvider.TAG, "Method channel does not exist")
        return null
    }

    val future = CompletableFuture<T?>()

    runInMainThread {
        channel.invokeMethod(method, arguments, object : MethodChannel.Result {
            override fun success(a: Any?) {
                future.complete(a as T?)
            }

            override fun error(errorCode: String, errorMessage: String?, errorDetails: Any?) {
                future.completeExceptionally(
                    Exception(channelMethodErrorMessage(errorCode, errorMessage, errorDetails))
                )
            }

            override fun notImplemented() {
                future.completeExceptionally(NotImplementedError("method '$method' not implemented"))
            }
        })
    }

    return future.get()
}

private fun runInMainThread(f: () -> Unit) {
    Handler(Looper.getMainLooper()).post(f)
}

private fun channelMethodErrorMessage(code: String?, message: String?, details: Any?): String =
    "error invoking channel method (code: $code, message: $message, details: $details)"

