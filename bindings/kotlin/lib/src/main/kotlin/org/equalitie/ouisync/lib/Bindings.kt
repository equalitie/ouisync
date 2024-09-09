package org.equalitie.ouisync.lib

import com.sun.jna.Library
import com.sun.jna.Native
import com.sun.jna.Pointer
import com.sun.jna.Structure
import com.sun.jna.Structure.FieldOrder
import com.sun.jna.Callback as JnaCallback

internal interface Bindings : Library {
    companion object {
        val INSTANCE: Bindings by lazy {
            Native.load("ouisync_ffi", Bindings::class.java)
        }
    }

    fun session_create(
        kind: Byte,
        configs_path: String,
        log_path: String?,
        log_tag: String,
        context: Pointer?,
        callback: Callback,
    ): SessionCreateResult

    fun session_close(handle: Handle, context: Pointer?, callback: Callback)

    fun session_channel_send(handle: Handle, msg: ByteArray, msg_len: Int)

    fun free_string(ptr: Pointer?)
}

internal typealias Handle = Long

interface Callback : JnaCallback {
    fun invoke(context: Pointer?, msg_ptr: Pointer, msg_len: Long)
}

@FieldOrder("handle", "error_code", "error_message")
internal class SessionCreateResult(
    @JvmField var handle: Handle = 0,
    @JvmField var error_code: Short = ErrorCode.OK.encode(),
    @JvmField var error_message: Pointer? = null,
) : Structure(), Structure.ByValue
