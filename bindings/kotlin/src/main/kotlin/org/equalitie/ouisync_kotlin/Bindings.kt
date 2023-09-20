package org.equalitie.ouisync_kotlin

import com.sun.jna.Callback as JnaCallback
import com.sun.jna.FromNativeContext
import com.sun.jna.Library
import com.sun.jna.Native
import com.sun.jna.NativeMapped
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

    fun session_destroy(handle: Handle)

    fun session_channel_send(handle: Handle, msg: ByteArray, msg_len: Int)

    fun free_string(ptr: Pointer?)
}

internal typealias Handle = Long

interface Callback : JnaCallback {
    fun invoke(context: Pointer?, msg_ptr: Pointer, msg_len: Long);
}

@FieldOrder("handle", "error_code", "error_message")
internal class SessionCreateResult(
    @JvmField var handle: Handle = 0,
    @JvmField var error_code: ErrorCode = ErrorCode.OK,
    @JvmField var error_message: Pointer? = null,
) : Structure(), Structure.ByValue

enum class ErrorCode : NativeMapped {
    OK,
    STORE,
    PERMISSION_DENIED,
    MALFORMED_DATA,
    ENTRY_EXISTS,
    ENTRY_NOT_FOUND,
    AMBIGUOUS_ENTRY,
    DIRECTORY_NOT_EMPTY,
    OPERATION_NOT_SUPPORTED,
    CONFIG,
    INVALID_ARGUMENT,
    MALFORMED_MESSAGE,
    STORAGE_VERSION_MISMATCH,
    CONNECTION_LOST,

    VFS_INVALID_MOUNT_POINT,
    VFS_DRIVER_INSTALL,
    VFS_BACKEND,

    OTHER;

    companion object {
        internal fun fromShort(n: Short): ErrorCode = when (n.toInt()) {
            0 -> OK
            1 -> STORE
            2 -> PERMISSION_DENIED
            3 -> MALFORMED_DATA
            4 -> ENTRY_EXISTS
            5 -> ENTRY_NOT_FOUND
            6 -> AMBIGUOUS_ENTRY
            7 -> DIRECTORY_NOT_EMPTY
            8 -> OPERATION_NOT_SUPPORTED
            10 -> CONFIG
            11 -> INVALID_ARGUMENT
            12 -> MALFORMED_MESSAGE
            13 -> STORAGE_VERSION_MISMATCH
            14 -> CONNECTION_LOST
            2048 -> VFS_INVALID_MOUNT_POINT
            2049 -> VFS_DRIVER_INSTALL
            2050 -> VFS_BACKEND
            else -> OTHER
        }
    }

    override fun fromNative(nativeValue: Any, context: FromNativeContext): Any =
        fromShort(nativeValue as Short)

    override fun toNative(): Any =
        when (this) {
            OK -> 0
            STORE -> 1
            PERMISSION_DENIED -> 2
            MALFORMED_DATA -> 3
            ENTRY_EXISTS -> 4
            ENTRY_NOT_FOUND -> 5
            AMBIGUOUS_ENTRY -> 6
            DIRECTORY_NOT_EMPTY -> 7
            OPERATION_NOT_SUPPORTED -> 8
            CONFIG -> 10
            INVALID_ARGUMENT -> 11
            MALFORMED_MESSAGE -> 12
            STORAGE_VERSION_MISMATCH -> 13
            CONNECTION_LOST -> 14
            VFS_INVALID_MOUNT_POINT -> 2048
            VFS_DRIVER_INSTALL -> 2049
            VFS_BACKEND -> 2050
            OTHER -> 65535
        }.toShort()

    override fun nativeType(): Class<Short> {
        return Short::class.java
    }
}

