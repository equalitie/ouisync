package org.equalitie.ouisync_kotlin

import com.sun.jna.Pointer

class Session private constructor(val handle: Long) {
    companion object {
        private val bindings = Bindings.INSTANCE

        fun create(configsPath: String, logPath: String): Session {
            val callback = object : Callback {
                override fun invoke(context: Pointer, msg_ptr: Pointer, msg_len: Int) {
                    // ...
                }
            }

            val result = bindings.session_create(
                configsPath,
                logPath,
                null,
                callback
            )

            if (result.error_code == 0.toShort()) {
                return Session(result.handle)
            } else {
                // TODO: deallocate the message
                throw Error(result.error_code, result.error_message!!)
            }
        }
    }

    fun dispose() {
        bindings.session_destroy(handle)
    }
}

class Error(code: Short, message: String) : Exception(message)