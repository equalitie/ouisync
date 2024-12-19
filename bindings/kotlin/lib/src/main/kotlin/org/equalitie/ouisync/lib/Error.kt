package org.equalitie.ouisync.lib

open class Error(val code: ErrorCode, message: String) : Exception(message) {
    companion object {
        fun fromCode(code: ErrorCode): Error = when (code) {
            ErrorCode.SERVICE_ALREADY_RUNNING -> ServiceAlreadyRunning(code.toString())
            else -> Error(code, code.toString())
        }
    }
}

class ServiceAlreadyRunning(message: String) : Error(ErrorCode.SERVICE_ALREADY_RUNNING, message)
