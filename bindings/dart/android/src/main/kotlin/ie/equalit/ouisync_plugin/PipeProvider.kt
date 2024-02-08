package ie.equalit.ouisync_plugin

import android.content.Context
import android.content.res.AssetFileDescriptor
import android.net.Uri
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.os.storage.StorageManager;
import android.util.Log
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.MethodChannel.Result
import java.io.FileNotFoundException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.Callable
import java.util.concurrent.FutureTask
import java.util.concurrent.Semaphore

class PipeProvider: AbstractFileProvider() {
    companion object {
        private const val CHUNK_SIZE = 64000
        private val TAG = PipeProvider::class.java.simpleName

        private val supportsProxyFileDescriptor: Boolean
            get() = android.os.Build.VERSION.SDK_INT >= 26
    }

    private var _handler: Handler? = null

    private val handler: Handler
        @Synchronized get() {
            if (_handler == null) {
                Log.d(TAG, "Creating worker thread")

                val thread = HandlerThread("${javaClass.simpleName} worker thread")
                thread.start();
                _handler = Handler(thread.getLooper())
            }

            return _handler!!
        }

    override fun onCreate(): Boolean {
        return true
    }

    // TODO: Handle `mode`
    @Throws(FileNotFoundException::class)
    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor? {
        Log.d(TAG, "Opening file '$uri' in mode '$mode'")

        val path = getPathFromUri(uri);
        Log.d(TAG, "Pipe path=" + path);
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

    @Throws(FileNotFoundException::class)
    private fun openProxyFile(path: String, size: Long): ParcelFileDescriptor? {
        var storage = context!!.getSystemService(Context.STORAGE_SERVICE) as StorageManager;

        // https://developer.android.google.cn/reference/android/os/storage/StorageManager
        return storage.openProxyFileDescriptor(
            ParcelFileDescriptor.MODE_READ_ONLY,
            ProxyCallbacks(path, size),
            handler
        )
    }

    @Throws(FileNotFoundException::class)
    private fun openPipe(path: String): ParcelFileDescriptor? {
        var pipe: Array<ParcelFileDescriptor?>?

        try {
            pipe = ParcelFileDescriptor.createPipe()
        } catch (e: IOException) {
            Log.e(TAG, "Exception opening pipe", e)
            throw FileNotFoundException("Could not open pipe for: $path")
        }

        var reader = pipe[0]
        var writer = pipe[1]
        var dstFd = writer!!.detachFd();

        runInUiThread {
            copyFileToRawFd(path, dstFd, object: MethodChannel.Result {
                override fun success(a: Any?) {
                    writer.close()
                }

                override fun error(code: String, message: String?, details: Any?) {
                    Log.e(TAG, channelMethodErrorMessage(code, message, details))
                    writer.close()
                }

                override fun notImplemented() {}
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
                id = invokeBlocking<Int> { result -> openFile(path, result) }
                  ?: throw FileNotFoundException("file not found: $path")
                this.id = id
            }

            val chunk = invokeBlocking<ByteArray> { result ->
                readFile(id, chunkSize, offset, result)
            }

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
                invokeBlocking<Unit> { result -> closeFile(id, result) }
            }
        }
    }
}

private fun openFile(path: String, result: MethodChannel.Result) {
    val arguments = hashMapOf<String, Any>("path" to path)
    OuisyncPlugin.invokeMethod("openFile", arguments, result)
}

private fun closeFile(id: Int, result: MethodChannel.Result) {
    val arguments = hashMapOf<String, Any>("id" to id)
    OuisyncPlugin.invokeMethod("closeFile", arguments, result)
}

private fun readFile(id: Int, chunkSize: Int, offset: Long, result: MethodChannel.Result) {
    val arguments = hashMapOf<String, Any>("id" to id, "chunkSize" to chunkSize, "offset" to offset)
    OuisyncPlugin.invokeMethod("readFile", arguments, result)
}

private fun copyFileToRawFd(srcPath: String, dstFd: Int, result: MethodChannel.Result) {
    val arguments = hashMapOf<String, Any>("srcPath" to srcPath, "dstFd" to dstFd)
    OuisyncPlugin.invokeMethod("copyFileToRawFd", arguments, result)
}

// Implementation of MethodChannel.Result which blocks until the result is available.
class BlockingResult<T>: MethodChannel.Result {
    private val semaphore = Semaphore(1)
    private var result: Any? = null

    init {
        semaphore.acquire(1)
    }

    // Wait until the result is available and returns it. If the invoked method failed, throws an
    // exception.
    fun wait(): T? {
        semaphore.acquire(1)

        try {
            val result = this.result

            if (result is Throwable) {
                throw result
            } else {
                return result as T?
            }
        } finally {
            semaphore.release(1)
        }
    }

    override fun success(a: Any?) {
        result = a
        semaphore.release(1)
    }

    override fun error(errorCode: String, errorMessage: String?, errorDetails: Any?) {
        result = Exception(channelMethodErrorMessage(errorCode, errorMessage, errorDetails))
        semaphore.release(1)
    }

    override fun notImplemented() {}
}

private fun <T> invokeBlocking(f: (MethodChannel.Result) -> Unit): T? {
    val result = BlockingResult<T>()
    runInUiThread { f(result) }
    return result.wait()
}

private fun runInUiThread(f: () -> Unit) {
    Handler(Looper.getMainLooper()).post { f() }
}

private fun channelMethodErrorMessage(code: String?, message: String?, details: Any?): String =
    "error invoking channel method (code: $code, message: $message, details: $details)"

