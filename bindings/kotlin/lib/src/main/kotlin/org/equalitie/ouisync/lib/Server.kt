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
            val handle = bindings.service_start(configPath, debugLabel, result, null)
            result.await()

            return Server(handle)
        }
    }

    suspend fun stop() {
        val result = ResultHandler()
        bindings.service_stop(handle, result, null)
        result.await()
    }
}

typealias LogFunction = (level: LogLevel, message: String) -> Unit

fun initLog(
    file: String? = null,
    callback: LogFunction? = null,
    tag: String = "",
) {
    bindings.log_init(
        file,
        callback?.let(::LogHandler),
        tag,
    )
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
    override fun invoke(level: Byte, message: String) {
        function(LogLevel.decode(level), message)
    }
}
