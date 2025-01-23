package org.equalitie.ouisync.lib

import com.sun.jna.Pointer
import kotlinx.coroutines.CompletableDeferred

private val bindings = Bindings.INSTANCE

class Server private constructor(private val handle: Pointer) {
    companion object {

        suspend fun start(
            configPath: String,
            debugLabel: String? = null,
        ): Server {
            val result = ResultHandler()
            val handle = bindings.start_service(configPath, debugLabel, result, null)
            result.await()

            return Server(handle)
        }
    }

    suspend fun stop() {
        val result = ResultHandler()
        bindings.stop_service(handle, result, null)
        result.await()
    }
}

typealias LogFunction = (level: LogLevel, message: String) -> Unit

// Need to keep the callback referenced to prevent it from being GC'd.
private var logHandler: LogHandler? = null

fun initLog(
    file: String? = null,
    callback: LogFunction? = null,
) {
    logHandler = logHandler ?: callback?.let(::LogHandler)
    bindings.init_log(file, logHandler)
}

private class ResultHandler() : StatusCallback {
    private val deferred = CompletableDeferred<Short>()

    override fun invoke(context: Pointer?, errorCode: Short) {
        deferred.complete(errorCode)
    }

    suspend fun await() {
        val errorCode = ErrorCode.decode(deferred.await())

        if (errorCode != ErrorCode.OK) {
            throw Error.dispatch(errorCode)
        }
    }
}

private class LogHandler(val function: LogFunction) : LogCallback {
    override fun invoke(level: Byte, ptr: Pointer, len: Long, cap: Long) {
        val level = LogLevel.decode(level)
        val message = ptr.getByteArray(0, len.toInt()).decodeToString()

        try {
            function(level, message)
        } finally {
            bindings.release_log_message(ptr, len, cap)
        }
    }
}
