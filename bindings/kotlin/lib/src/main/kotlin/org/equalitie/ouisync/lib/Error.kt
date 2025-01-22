package org.equalitie.ouisync.lib

open class Error internal constructor(
    val code: ErrorCode,
    message: String?,
    val sources: List<String> = emptyList(),
) : Exception(message ?: code.toString()) {
    companion object {
        internal fun dispatch(
            code: ErrorCode,
            message: String? = null,
            sources: List<String> = emptyList(),
        ): Error = when (code) {
            ErrorCode.PERMISSION_DENIED -> PermissionDenied(message, sources)
            ErrorCode.INVALID_DATA -> InvalidData(message, sources)
            ErrorCode.NOT_FOUND -> NotFound(message, sources)
            ErrorCode.STORE_ERROR -> StoreError(message, sources)
            ErrorCode.SERVICE_ALREADY_RUNNING -> ServiceAlreadyRunning(message, sources)
            ErrorCode.UNSUPPORTED -> Unsupported(message, sources)
            else -> Error(code, message, sources)
        }
    }

    override fun toString() = "${this::class.simpleName}: ${(sequenceOf(message) + sources.asSequence()).joinToString(" → ")}"

    class PermissionDenied(message: String? = null, sources: List<String> = emptyList()) :
        Error(ErrorCode.PERMISSION_DENIED, message, sources)

    class InvalidData(message: String? = null, sources: List<String> = emptyList()) :
        Error(ErrorCode.INVALID_DATA, message, sources)

    class NotFound(message: String? = null, sources: List<String> = emptyList()) :
        Error(ErrorCode.NOT_FOUND, message, sources)

    class StoreError(message: String? = null, sources: List<String> = emptyList()) :
        Error(ErrorCode.STORE_ERROR, message, sources)

    class ServiceAlreadyRunning(message: String? = null, sources: List<String> = emptyList()) :
        Error(ErrorCode.SERVICE_ALREADY_RUNNING, message, sources)

    class Unsupported(message: String? = null, sources: List<String> = emptyList()) : Error(ErrorCode.UNSUPPORTED, message, sources)
}
