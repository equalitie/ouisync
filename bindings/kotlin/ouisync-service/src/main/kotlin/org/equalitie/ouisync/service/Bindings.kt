package org.equalitie.ouisync.service

import com.sun.jna.Library
import com.sun.jna.Native
import com.sun.jna.Pointer
import com.sun.jna.Callback as JnaCallback

@Suppress("ktlint:standard:function-naming")
internal interface Bindings : Library {
    companion object {
        val INSTANCE: Bindings by lazy { Native.load("ouisync_service", Bindings::class.java) }
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

    fun init_log()
}

internal interface StatusCallback : JnaCallback {
    fun callback(context: Pointer?, error_code: Short)
}
