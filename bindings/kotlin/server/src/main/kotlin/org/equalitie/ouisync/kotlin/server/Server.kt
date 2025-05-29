package org.equalitie.ouisync.kotlin.server

import com.sun.jna.Pointer
import kotlinx.coroutines.suspendCancellableCoroutine
import org.equalitie.ouisync.kotlin.client.ErrorCode
import org.equalitie.ouisync.kotlin.client.LogLevel
import org.equalitie.ouisync.kotlin.client.OuisyncException
import kotlin.coroutines.Continuation
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlin.coroutines.suspendCoroutine

private val bindings = Bindings.INSTANCE

class Server private constructor(private val handle: Pointer) {
    companion object {
        suspend fun start(
            configPath: String,
            debugLabel: String? = null,
        ): Server {
            var handle = Pointer.NULL

            suspendCancellableCoroutine<Unit> { cont ->
                handle =
                    bindings.start_service(
                        configPath,
                        debugLabel,
                        CoroutineHandler(cont),
                        null,
                    )

                cont.invokeOnCancellation { bindings.stop_service(handle, NoopHandler, null) }
            }

            return Server(handle)
        }
    }

    suspend fun stop() = suspendCoroutine<Unit> { cont -> bindings.stop_service(handle, CoroutineHandler(cont), null) }
}

private class CoroutineHandler(val cont: Continuation<Unit>) : StatusCallback {
    override fun invoke(context: Pointer?, error_code: Short) {
        val errorCode = ErrorCode.fromValue(error_code)

        if (errorCode == ErrorCode.OK) {
            cont.resume(Unit)
        } else {
            cont.resumeWithException(OuisyncException.dispatch(errorCode))
        }
    }
}

private object NoopHandler : StatusCallback {
    override fun invoke(context: Pointer?, error_code: Short) = Unit
}

typealias LogFunction = (level: LogLevel, message: String) -> Unit

// Need to keep the callback referenced to prevent it from being GC'd.
private var logHandler: LogHandler? = null

fun initLog(
    stdout: Boolean = false,
    file: String? = null,
    callback: LogFunction? = null,
) {
    logHandler = logHandler ?: callback?.let(::LogHandler)
    bindings.init_log(if (stdout) 1 else 0, file, logHandler)
}

private class LogHandler(val function: LogFunction) : LogCallback {
    override fun invoke(level: Byte, ptr: Pointer, len: Long, cap: Long) {
        val level = LogLevel.fromValue(level)
        val message = ptr.getByteArray(0, len.toInt()).decodeToString()

        try {
            function(level, message)
        } finally {
            bindings.release_log_message(ptr, len, cap)
        }
    }
}
