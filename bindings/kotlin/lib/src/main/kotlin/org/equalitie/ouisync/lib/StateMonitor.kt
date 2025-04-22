package org.equalitie.ouisync.lib

import kotlinx.serialization.KSerializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder

@Serializable(with = IdSerializer::class)
data class MonitorId(val name: String, val disambiguator: Long) {
    override fun toString() = "$name:$disambiguator"
}

@Serializable
class StateMonitorNode(val values: Map<String, String>, val children: List<MonitorId>) {
    // companion object {
    //     fun decode(u: MessageUnpacker): StateMonitorNode {
    //         if (u.unpackArrayHeader() != 2) {
    //             throw DecodeError()
    //         }

    //         val values = decodeValues(u)
    //         val children = decodeChildren(u)

    //         return StateMonitorNode(values, children)
    //     }

    //     private fun decodeValues(u: MessageUnpacker): Map<String, String> {
    //         val n = u.unpackMapHeader()
    //         return buildMap {
    //             repeat(n) {
    //                 val k = u.unpackString()
    //                 val v = u.unpackString()
    //                 put(k, v)
    //             }
    //         }
    //     }

    //     private fun decodeChildren(u: MessageUnpacker): List<MonitorId> {
    //         val n = u.unpackArrayHeader()
    //         return buildList {
    //             repeat(n) {
    //                 add(MonitorId.decode(u))
    //             }
    //         }
    //     }
    // }
}

private object IdSerializer : KSerializer<MonitorId> {
    override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor(
        MonitorId::class.qualifiedName!!,
        PrimitiveKind.STRING,
    )

    override fun serialize(encoder: Encoder, value: MonitorId) {
        encoder.encodeString(value.toString())
    }

    override fun deserialize(decoder: Decoder): MonitorId {
        val raw = decoder.decodeString()

        // A string in the format "name:disambiguator".
        val colon = raw.lastIndexOf(':')
        val name = raw.substring(0..colon)
        val disambiguator = raw.substring(colon + 1).toLong()

        return MonitorId(name, disambiguator)
    }
}
