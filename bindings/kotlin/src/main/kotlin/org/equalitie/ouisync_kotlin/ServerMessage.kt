package org.equalitie.ouisync_kotlin

import org.msgpack.core.MessageUnpacker
import org.msgpack.value.ValueType

internal sealed interface ServerMessage {
    companion object {
        fun unpack(unpacker: MessageUnpacker): ServerMessage {
            if (unpacker.getNextFormat().getValueType() != ValueType.MAP) {
                throw InvalidMessage()
            }

            if (unpacker.unpackMapHeader() < 1) {
                throw InvalidMessage()
            }

            val kind = unpacker.unpackString()

            when (kind) {
                "success" -> return Success.unpack(unpacker)
                "failure" -> return Failure.unpack(unpacker)
                "notification" -> return Notification.unpack(unpacker)
                else -> throw InvalidMessage()
            }
        }
    }
}

internal sealed interface Response : ServerMessage

internal class Success(val value: Any?) : Response {
    companion object {
        fun unpack(unpacker: MessageUnpacker): Response {
            when (unpacker.getNextFormat().getValueType()) {
                ValueType.STRING -> {
                    if (unpacker.unpackString() == "none") {
                        return Success(null)
                    } else {
                        throw InvalidResponse()
                    }
                }
                ValueType.MAP -> {
                    if (unpacker.unpackMapHeader() < 1) {
                        throw InvalidResponse()
                    }

                    val name = unpacker.unpackString()
                    val value = unpackValue(name, unpacker)

                    return Success(value)
                }
                else -> throw InvalidResponse()
            }
        }

        private fun unpackValue(name: String, unpacker: MessageUnpacker): Any? =
            when (unpacker.getNextFormat().getValueType()) {
                ValueType.STRING -> unpacker.unpackString()
                else -> throw InvalidResponse()
            }

    }
}

internal class Failure(val error: Error) : Response {
    companion object {
        fun unpack(unpacker: MessageUnpacker): Response {
            if (unpacker.getNextFormat().getValueType() != ValueType.ARRAY) {
                return Failure(Error(ErrorCode.MALFORMED_MESSAGE, "invalid error response"))
            }

            if (unpacker.unpackArrayHeader() < 2) {
                return Failure(Error(ErrorCode.MALFORMED_MESSAGE, "invalid error response"))
            }

            try {
                val code = ErrorCode.fromShort(unpacker.unpackShort())
                val message = unpacker.unpackString()

                return Failure(Error(code, message))
            } catch (e: Exception) {
                return Failure(Error(ErrorCode.MALFORMED_MESSAGE, "invalid error response"))
            }
        }
    }
}

internal class Notification(val content: Any) : ServerMessage {
    companion object {
        fun unpack(unpacker: MessageUnpacker): Notification {
            throw Exception("TODO")
        }
    }
}

internal open class InvalidMessage : Exception {
    constructor() : super("invalid message")
    constructor(message: String) : super(message)
}

internal class InvalidResponse : InvalidMessage("invalid response")


// pub(crate) enum Response {
//     None,
//     Bool(bool),
//     U8(u8),
//     U32(u32),
//     U64(u64),
//     Bytes(#[serde(with = "serde_bytes")] Vec<u8>),
//     String(String),
//     Handle(u64),
//     Directory(Directory),
//     StateMonitor(StateMonitor),
//     Progress(Progress),
//     PeerInfo(Vec<PeerInfo>),
// }
