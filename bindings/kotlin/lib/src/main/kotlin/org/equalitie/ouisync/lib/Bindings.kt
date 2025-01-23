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

    fun start_service(
        config_dir: String,
        debug_label: String?,
        callback: StatusCallback,
        callback_context: Pointer?,
    ): Pointer

    fun stop_service(
        handle: Pointer,
        callback: StatusCallback,
        callback_context: Pointer?,
    )

    fun init_log(
        file: String?,
        callback: LogCallback?,
    ): Short

    fun release_log_message(ptr: Pointer, len: Long, cap: Long)
}

internal typealias Handle = Long

internal interface StatusCallback : JnaCallback {
    fun invoke(context: Pointer?, error_code: Short)
}

internal interface LogCallback : JnaCallback {
    fun invoke(level: Byte, ptr: Pointer, len: Long, cap: Long)
}
