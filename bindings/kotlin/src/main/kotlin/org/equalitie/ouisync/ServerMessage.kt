package org.equalitie.ouisync

import org.msgpack.core.MessageUnpacker
import org.msgpack.value.ValueType

data class PeerInfo(
    val ip: String,
    val port: UShort,
    val source: String,
    val state: String,
    val runtimeId: String?,
) {
    companion object {
        fun unpack(unpacker: MessageUnpacker): PeerInfo {
            val count = unpacker.unpackArrayHeader()

            if (count < 4) {
                throw InvalidResponse()
            }

            val ip = unpacker.unpackString()
            val port = unpacker.unpackInt().toUShort()
            val source = unpacker.unpackString()

            var state: String
            var runtimeId: String? = null

            when (unpacker.getNextFormat().getValueType()) {
                ValueType.STRING -> {
                    state = unpacker.unpackString()
                }
                ValueType.MAP -> {
                    if (unpacker.unpackMapHeader() < 1) {
                        throw InvalidResponse()
                    }

                    state = unpacker.unpackString()

                    val length = unpacker.unpackBinaryHeader()
                    val runtimeIdBytes = unpacker.readPayload(length)

                    @OptIn(kotlin.ExperimentalStdlibApi::class)
                    runtimeId = runtimeIdBytes.toHexString()
                }
                else -> throw InvalidResponse()
            }

            return PeerInfo(ip, port, source, state, runtimeId)
        }
    }
}

enum class NetworkEvent {
    PROTOCOL_VERSION_MISMATCH, // 0
    PEER_SET_CHANGE, // = 1

    ;

    companion object {
        fun fromByte(n: Byte): NetworkEvent = when (n.toInt()) {
            0 -> PROTOCOL_VERSION_MISMATCH
            1 -> PEER_SET_CHANGE
            else -> throw IllegalArgumentException()
        }
    }

    fun toByte(): Byte = when (this) {
        PROTOCOL_VERSION_MISMATCH -> 0
        PEER_SET_CHANGE -> 1
    }
}

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
                    if (unpacker.unpackMapHeader() != 1) {
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
                ValueType.ARRAY -> {
                    when (name) {
                        "peer_info" -> unpackPeerInfo(unpacker)
                        else -> throw InvalidResponse()
                    }
                }
                ValueType.BOOLEAN -> unpacker.unpackBoolean()
                ValueType.INTEGER -> {
                    when (name) {
                        "u8" -> unpacker.unpackByte()
                        "u32" -> unpacker.unpackInt()
                        "u64", "handle" -> unpacker.unpackLong()
                        else -> throw InvalidResponse()
                    }
                }
                ValueType.STRING -> unpacker.unpackString()
                else -> throw InvalidResponse()
            }

        private fun unpackPeerInfo(unpacker: MessageUnpacker): List<PeerInfo> {
            val count = unpacker.unpackArrayHeader()
            return 0.rangeUntil(count).map { PeerInfo.unpack(unpacker) }
        }
    }
}

internal class Failure(val error: Error) : Response {
    companion object {
        private val INVALID_ERROR = Error(ErrorCode.MALFORMED_MESSAGE, "invalid error response")

        fun unpack(unpacker: MessageUnpacker): Response {
            if (unpacker.getNextFormat().getValueType() != ValueType.ARRAY) {
                return Failure(INVALID_ERROR)
            }

            if (unpacker.unpackArrayHeader() != 2) {
                return Failure(INVALID_ERROR)
            }

            try {
                val code = ErrorCode.fromShort(unpacker.unpackShort())
                val message = unpacker.unpackString()

                return Failure(Error(code, message))
            } catch (e: Exception) {
                return Failure(INVALID_ERROR)
            }
        }
    }
}

internal class Notification(val content: Any?) : ServerMessage {
    companion object {
        fun unpack(unpacker: MessageUnpacker): Notification {
            val name = when (unpacker.getNextFormat().getValueType()) {
                ValueType.STRING -> unpacker.unpackString()
                ValueType.MAP -> {
                    if (unpacker.unpackMapHeader() != 1) {
                        throw InvalidNotification()
                    }

                    unpacker.unpackString()
                }
                else -> throw InvalidNotification()
            }

            val value = unpackValue(name, unpacker)

            return Notification(value)
        }

        private fun unpackValue(name: String, unpacker: MessageUnpacker): Any? {
            when (name) {
                "network" -> return NetworkEvent.fromByte(unpacker.unpackByte())
                else -> throw InvalidNotification()
            }
        }
    }
}

internal open class InvalidMessage : Exception {
    constructor() : super("invalid message")
    constructor(message: String) : super(message)
}

internal class InvalidResponse : InvalidMessage("invalid response")
internal class InvalidNotification : InvalidMessage("invalid notification")

// pub(crate) enum Response {
//     Bytes(#[serde(with = "serde_bytes")] Vec<u8>),
//     Directory(Directory),
//     StateMonitor(StateMonitor),
//     Progress(Progress),
// }
