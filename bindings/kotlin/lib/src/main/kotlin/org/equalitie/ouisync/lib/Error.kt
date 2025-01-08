package org.equalitie.ouisync.lib

open class Error internal constructor(val code: ErrorCode, message: String?) : Exception(message ?: code.toString()) {
    companion object {
        internal fun dispatch(code: ErrorCode, message: String? = null): Error = when (code) {
            ErrorCode.PERMISSION_DENIED -> PermissionDenied(message)
            ErrorCode.INVALID_DATA -> InvalidData(message)
            ErrorCode.SERVICE_ALREADY_RUNNING -> ServiceAlreadyRunning(message)
            else -> Error(code, message)
        }
    }
}

class PermissionDenied(message: String? = null) : Error(ErrorCode.PERMISSION_DENIED, message)

class InvalidData(message: String? = null) : Error(ErrorCode.INVALID_DATA, message)

class ServiceAlreadyRunning(message: String? = null) : Error(ErrorCode.SERVICE_ALREADY_RUNNING, message)
