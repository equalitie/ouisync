package org.equalitie.ouisync

import org.msgpack.core.MessageUnpacker
import org.msgpack.value.ValueType

/**
 * Information about a peer.
 *
 * @property addr      remote address of the peer in the "PROTOCOL/IP:PORT" format.
 * @property source    how was the peer discovered.
 * @property state     state of the peer connection.
 * @property runtimeId [runtime id][Session.thisRuntimeId] of the peer if [active][PeerStateKind.ACTIVE], otherwise null.
 */
data class PeerInfo(
    val addr: String,
    val source: PeerSource,
    val state: PeerStateKind,
    val runtimeId: String?,
) {
    companion object {
        fun unpack(unpacker: MessageUnpacker): PeerInfo {
            val count = unpacker.unpackArrayHeader()

            if (count < 3) {
                throw InvalidResponse()
            }

            val addr = unpacker.unpackString()
            val source = PeerSource.decode(unpacker.unpackByte())

            var state: PeerStateKind
            var runtimeId: String? = null

            when (unpacker.getNextFormat().getValueType()) {
                ValueType.INTEGER -> {
                    state = PeerStateKind.decode(unpacker.unpackByte())
                }
                ValueType.ARRAY -> {
                    if (unpacker.unpackArrayHeader() < 2) {
                        throw InvalidResponse()
                    }

                    state = PeerStateKind.decode(unpacker.unpackByte())

                    val length = unpacker.unpackBinaryHeader()
                    val runtimeIdBytes = unpacker.readPayload(length)

                    @OptIn(kotlin.ExperimentalStdlibApi::class)
                    runtimeId = runtimeIdBytes.toHexString()
                }
                else -> throw InvalidResponse()
            }

            return PeerInfo(addr, source, state, runtimeId)
        }
    }
}

data class Progress(val value: Long, val total: Long) {
    companion object {
        fun unpack(unpacker: MessageUnpacker): Progress {
            if (unpacker.unpackArrayHeader() != 2) {
                throw InvalidResponse()
            }

            val value = unpacker.unpackLong()
            val total = unpacker.unpackLong()

            return Progress(value, total)
        }
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

        private fun unpackValue(name: String, unpacker: MessageUnpacker): Any =
            when (unpacker.getNextFormat().getValueType()) {
                ValueType.ARRAY -> {
                    when (name) {
                        "directory" -> Directory.unpack(unpacker)
                        "peer_infos" -> unpackPeerInfos(unpacker)
                        "progress" -> Progress.unpack(unpacker)
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
                ValueType.BINARY -> {
                    val length = unpacker.unpackBinaryHeader()
                    unpacker.readPayload(length)
                }
                else -> throw InvalidResponse()
            }

        private fun unpackPeerInfos(unpacker: MessageUnpacker): List<PeerInfo> {
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
                val code = ErrorCode.decode(unpacker.unpackInt().toShort())
                val message = unpacker.unpackString()

                return Failure(Error(code, message))
            } catch (e: Exception) {
                println("Failure.unpack e $e")
                return Failure(INVALID_ERROR)
            }
        }
    }
}

internal class Notification(val content: Any) : ServerMessage {
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

        private fun unpackValue(name: String, unpacker: MessageUnpacker): Any {
            when (name) {
                "network" -> return NetworkEvent.decode(unpacker.unpackByte())
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
