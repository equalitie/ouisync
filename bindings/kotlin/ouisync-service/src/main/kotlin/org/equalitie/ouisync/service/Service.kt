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

/**
 * Service manages the repositories and runs the sync protocol. It can be interacted with using
 * [Session][org.equalitie.ouisync.session.Session].
 *
 * Note: to create a service, use [Service.start].
 */
class Service private constructor(private var handle: Pointer?) {
    companion object {
        /**
         * Starts the service.
         *
         * @param configPath path to the config directory of this service. If it doesn't exist, it's
         *   created automatically. The service requires both read and write access to it.
         * @param debugLabel Optional label used to distinguish mutliple services running in the same
         *   process. Used mainly for testing and debugging the library itself.
         */
        suspend fun start(
            configPath: String,
            debugLabel: String? = null,
        ): Service {
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

            return Service(handle)
        }
    }

    /** Stops this service. Has no effect if the service has already been stopped. */
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

/**
 * Enables logging of Ouisync's internal messages using
 * [Android log API](https://developer.android.com/reference/android/util/Log)
 *
 * Calling this function more than once has no effect. Currently there is no way to disable the
 * logging once it's been enabled.
 */
fun initLog() = bindings.init_log()
