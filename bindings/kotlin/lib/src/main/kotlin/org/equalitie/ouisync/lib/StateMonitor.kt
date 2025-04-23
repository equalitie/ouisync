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
class StateMonitorNode(val values: Map<String, String>, val children: List<MonitorId>)

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
