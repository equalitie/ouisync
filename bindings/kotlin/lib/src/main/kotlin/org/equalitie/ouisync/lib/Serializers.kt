@file:OptIn(kotlinx.serialization.ExperimentalSerializationApi::class)

package org.equalitie.ouisync.lib

import kotlinx.datetime.Instant
import kotlinx.serialization.KSerializer
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.descriptors.buildClassSerialDescriptor
import kotlinx.serialization.descriptors.element
import kotlinx.serialization.descriptors.listSerialDescriptor
import kotlinx.serialization.descriptors.nullable
import kotlinx.serialization.descriptors.serialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.encoding.decodeStructure
import kotlinx.serialization.serializer
import kotlin.time.Duration
import kotlin.time.Duration.Companion.milliseconds

/**
 * Serialize [Duration] as the number of whole milliseconds
 */
internal object DurationSerializer : KSerializer<Duration> {
    override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor(
        "${javaClass.getPackage()?.getName()}.Duration",
        PrimitiveKind.LONG,
    )

    override fun serialize(encoder: Encoder, value: Duration) {
        encoder.encodeLong(value.inWholeMilliseconds)
    }

    override fun deserialize(decoder: Decoder): Duration {
        return decoder.decodeLong().milliseconds
    }
}

/**
 * Serialize [Instant] as the number of milliseconds since epoch
 */
internal object InstantSerializer : KSerializer<Instant> {
    override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor(
        "${javaClass.getPackage()?.getName()}.Instant",
        PrimitiveKind.LONG,
    )

    override fun serialize(encoder: Encoder, value: Instant) {
        encoder.encodeLong(value.toEpochMilliseconds())
    }

    override fun deserialize(decoder: Decoder): Instant {
        return Instant.fromEpochMilliseconds(decoder.decodeLong())
    }
}

/**
 * Serializer for [OuisyncException]
 */
internal object OuisyncExceptionSerializer : KSerializer<OuisyncException> {
    override val descriptor: SerialDescriptor = buildClassSerialDescriptor(
        "${javaClass.getPackage()?.getName()}.OuisyncException",
    ) {
        element<ErrorCode>("code")
        element("message", serialDescriptor<String>().nullable)
        element("sources", listSerialDescriptor<String>())
    }

    override fun serialize(encoder: Encoder, value: OuisyncException) {
        throw NotImplementedError()
    }

    override fun deserialize(decoder: Decoder): OuisyncException =
        decoder.decodeStructure(descriptor) {
            val code = decodeSerializableElement<ErrorCode>(descriptor, 0, serializer())
            val message = decodeNullableSerializableElement<String>(descriptor, 1, serializer())
            val sources = decodeSerializableElement<List<String>>(descriptor, 2, serializer())

            OuisyncException.dispatch(code, message, sources)
        }
}
