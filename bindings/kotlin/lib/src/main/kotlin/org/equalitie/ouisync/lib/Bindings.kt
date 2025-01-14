package org.equalitie.ouisync.lib

import com.sun.jna.Library
import com.sun.jna.Native
import com.sun.jna.Pointer
import com.sun.jna.Callback as JnaCallback

@Suppress("ktlint:standard:function-naming")
internal interface Bindings : Library {
    companion object {
        val INSTANCE: Bindings by lazy {
            Native.load("ouisync_service", Bindings::class.java)
        }
    }

    fun service_start(
        config_dir: String,
        debug_label: String?,
        callback: StatusCallback,
        callback_context: Pointer?,
    ): Pointer

    fun service_stop(
        handle: Pointer,
        callback: StatusCallback,
        callback_context: Pointer?,
    )

    fun log_init(
        file: String?,
        callback: LogCallback?,
        tag: String,
    ): Short
}

internal typealias Handle = Long

interface StatusCallback : JnaCallback {
    fun invoke(context: Pointer?, error_code: Short)
}

interface LogCallback : JnaCallback {
    fun invoke(level: Byte, message: String)
}
