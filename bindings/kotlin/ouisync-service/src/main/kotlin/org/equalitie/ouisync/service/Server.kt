package org.equalitie.ouisync.service

import com.sun.jna.Pointer
import kotlinx.coroutines.suspendCancellableCoroutine
import org.equalitie.ouisync.session.ErrorCode
import org.equalitie.ouisync.session.OuisyncException
import kotlin.coroutines.Continuation
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlin.coroutines.suspendCoroutine

private val bindings = Bindings.INSTANCE

class Server private constructor(private var handle: Pointer?) {
    companion object {
        suspend fun start(
            configPath: String,
            debugLabel: String? = null,
        ): Server {
            var handle: Pointer? = null

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

    suspend fun stop() {
        val handle = this.handle
        if (handle == null) {
            return
        }

        this.handle = null

        suspendCoroutine<Unit> { cont -> bindings.stop_service(handle, CoroutineHandler(cont), null) }
    }
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

fun initLog() = bindings.init_log()
