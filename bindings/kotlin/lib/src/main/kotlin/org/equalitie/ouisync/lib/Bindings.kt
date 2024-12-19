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

    fun ouisync_start(
        socket_path: String,
        config_dir: String,
        debug_label: String?,
        callback: Callback,
        callback_context: Pointer?,
    ): Pointer

    fun ouisync_stop(
        handle: Pointer,
        callback: Callback,
        callback_context: Pointer?,
    )

    fun ouisync_log_init(
        log_file: String?,
        log_tag: String,
    ): Short

    fun ouisync_log_print(
        level: Byte,
        scope: String,
        message: String,
    )
}

internal typealias Handle = Long

interface Callback : JnaCallback {
    fun invoke(context: Pointer?, error_code: Short)
}
