package org.equalitie.ouisync.lib

import com.sun.jna.Pointer
import kotlinx.coroutines.CompletableDeferred

private val bindings = Bindings.INSTANCE

class Server private constructor(private val handle: Pointer) {
    companion object {

        suspend fun start(
            socketPath: String,
            configPath: String,
            debugLabel: String? = null,
        ): Server {
            val result = ResultHandler()
            val handle = bindings.ouisync_start(socketPath, configPath, debugLabel, result, null)
            result.await()

            return Server(handle)
        }
    }

    suspend fun stop() {
        val result = ResultHandler()
        bindings.ouisync_stop(handle, result, null)
        result.await()
    }
}

private class ResultHandler() : Callback {
    private val deferred = CompletableDeferred<Short>()

    override fun invoke(context: Pointer?, errorCode: Short) {
        deferred.complete(errorCode)
    }

    suspend fun await() {
        val errorCode = ErrorCode.decode(deferred.await())

        if (errorCode != ErrorCode.OK) {
            throw Error.fromCode(errorCode)
        }
    }
}

fun initLog(logFile: String? = null, logTag: String = "") {
    bindings.ouisync_log_init(logFile, logTag)
}
