package org.equalitie.ouisync_kotlin

import com.sun.jna.Callback as JnaCallback
import com.sun.jna.Library
import com.sun.jna.Native
import com.sun.jna.Pointer
import com.sun.jna.Structure
import com.sun.jna.Structure.FieldOrder
import com.sun.jna.Structure.StructField
import com.sun.jna.TypeMapper

internal interface Bindings : Library {
    companion object {
        val INSTANCE: Bindings by lazy {
            Native.load("ouisync_ffi", Bindings::class.java)
        }
    }

    fun session_create(
        configs_path: String,
        log_path: String?,
        context: Pointer?,
        callback: Callback,
    ): SessionCreateResult

    fun session_destroy(handle: Long)
}

interface Callback : JnaCallback {
    fun invoke(context: Pointer, msg_ptr: Pointer, msg_len: Int);
}

@FieldOrder("handle", "error_code", "error_message")
internal class SessionCreateResult(
    @JvmField var handle: Long = 0,
    @JvmField var error_code: Short = 0,
    @JvmField var error_message: String? = null,
) : Structure(), Structure.ByValue
