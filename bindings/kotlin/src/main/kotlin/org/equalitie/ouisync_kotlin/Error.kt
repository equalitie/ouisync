package org.equalitie.ouisync_kotlin

import org.msgpack.core.MessageUnpacker
import org.msgpack.value.ValueType

class Error(code: ErrorCode, message: String) : Exception(message) {
    companion object {
        internal fun unpack(unpacker: MessageUnpacker): Error {
            if (unpacker.getNextFormat().getValueType() != ValueType.ARRAY) {
                return Error(ErrorCode.MALFORMED_MESSAGE, "invalid error response")
            }

            if (unpacker.unpackArrayHeader() < 2) {
                return Error(ErrorCode.MALFORMED_MESSAGE, "invalid error response")
            }

            try {
                val code = ErrorCode.fromShort(unpacker.unpackShort())
                val message = unpacker.unpackString()

                return Error(code, message)
            } catch (e: Exception) {
                return Error(ErrorCode.MALFORMED_MESSAGE, "invalid error response")
            }
        }
    }
}
