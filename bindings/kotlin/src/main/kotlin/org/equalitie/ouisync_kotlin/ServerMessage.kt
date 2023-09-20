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
                "success" -> return Response.unpack(unpacker)
                "failure" -> throw Error.unpack(unpacker)
                "notification" -> return Notification.unpack(unpacker)
                else -> throw InvalidMessage()
            }
        }
    }
}

internal class Response(val content: Any?) : ServerMessage {
    companion object {
        fun unpack(unpacker: MessageUnpacker): Response {
            when (unpacker.getNextFormat().getValueType()) {
                ValueType.STRING -> {
                    if (unpacker.unpackString() == "none") {
                        return Response(null)
                    } else {
                        throw InvalidResponse()
                    }
                }
                ValueType.MAP -> {
                    throw Exception("TODO")
                }
                else -> throw InvalidResponse()
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
