@file:OptIn(kotlinx.serialization.ExperimentalSerializationApi::class)

package org.equalitie.ouisync.lib

import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.SerialInfo
import kotlinx.serialization.SerializationException
import kotlinx.serialization.SerializationStrategy
import kotlinx.serialization.builtins.ByteArraySerializer
import kotlinx.serialization.descriptors.PolymorphicKind
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.descriptors.StructureKind
import kotlinx.serialization.descriptors.elementDescriptors
import kotlinx.serialization.encoding.AbstractDecoder
import kotlinx.serialization.encoding.AbstractEncoder
import kotlinx.serialization.encoding.CompositeDecoder
import kotlinx.serialization.encoding.CompositeEncoder
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.modules.EmptySerializersModule
import kotlinx.serialization.modules.SerializersModule
import kotlinx.serialization.serializer
import org.msgpack.core.MessageBufferPacker
import org.msgpack.core.MessagePack
import org.msgpack.core.MessageUnpacker
import org.msgpack.value.ValueType
import kotlin.collections.firstNotNullOfOrNull

internal fun <T> encode(serializer: SerializationStrategy<T>, value: T): ByteArray {
    val encoder = MessageEncoder()
    encoder.encodeSerializableValue(serializer, value)
    return encoder.output
}

internal inline fun <reified T> encode(value: T): ByteArray = encode(serializer(), value)

internal fun <T> decode(serializer: DeserializationStrategy<T>, buffer: ByteArray): T {
    val decoder = MessageDecoder(buffer)
    return decoder.decodeSerializableValue(serializer)
}

internal inline fun <reified T> decode(buffer: ByteArray): T = decode(serializer(), buffer)

/**
 * Annotation for enum constants to serialize/deserialize that constant as the specified value
 * instead of its ordinal value.
 */
@SerialInfo
@Target(AnnotationTarget.FIELD)
annotation class Value(val value: Int)

internal class MessageEncoder(
    private val packer: MessageBufferPacker = MessagePack.newDefaultBufferPacker(),
    private val inSealed: Boolean = false,
) : AbstractEncoder() {
    val output: ByteArray
        get() = packer.toByteArray()

    override val serializersModule: SerializersModule = EmptySerializersModule()

    override fun encodeNull() {
        packer.packNil()
    }

    override fun encodeEnum(descriptor: SerialDescriptor, index: Int) {
        val annotations = descriptor.getElementAnnotations(index)
        val value = annotations.firstNotNullOfOrNull {
            if (it is Value) it.value else null
        } ?: index

        encodeInt(value)
    }

    override fun encodeBoolean(value: Boolean) {
        packer.packBoolean(value)
    }

    override fun encodeLong(value: Long) {
        packer.packLong(value)
    }

    override fun encodeInt(value: Int) {
        packer.packInt(value)
    }

    override fun encodeShort(value: Short) {
        packer.packShort(value)
    }

    override fun encodeByte(value: Byte) {
        packer.packByte(value)
    }

    override fun encodeString(value: String) {
        if (inSealed) return

        packer.packString(value)
    }

    override fun encodeInline(descriptor: SerialDescriptor): Encoder {
        if (inSealed) {
            packer.packMapHeader(1)
            packer.packString(unqualify(descriptor.serialName))
        }

        return MessageEncoder(packer)
    }

    override fun beginStructure(descriptor: SerialDescriptor): CompositeEncoder {
        when (descriptor.kind) {
            PolymorphicKind.SEALED -> return MessageEncoder(packer, inSealed = true)
            StructureKind.CLASS -> {
                if (inSealed) {
                    packer.packMapHeader(1)
                    packer.packString(unqualify(descriptor.serialName))
                }

                packer.packArrayHeader(descriptor.elementsCount)
            }
            StructureKind.OBJECT -> {
                if (inSealed) {
                    packer.packString(unqualify(descriptor.serialName))
                }
            }
            else -> Unit
        }

        return MessageEncoder(packer)
    }

    override fun beginCollection(descriptor: SerialDescriptor, size: Int): CompositeEncoder {
        when (descriptor.kind) {
            StructureKind.LIST -> packer.packArrayHeader(size)
            StructureKind.CLASS, StructureKind.OBJECT, StructureKind.MAP -> packer.packMapHeader(size)
            else -> SerializationException("wrong collection kind ${descriptor.kind}")
        }

        return MessageEncoder(packer)
    }

    override fun <T> encodeSerializableValue(serializer: SerializationStrategy<T>, value: T) {
        if (serializer == ByteArraySerializer()) {
            encodeByteArray(value as ByteArray)
        } else {
            super.encodeSerializableValue(serializer, value)
        }
    }

    fun encodeByteArray(value: ByteArray) {
        packer.packBinaryHeader(value.size)
        packer.addPayload(value)
    }
}

internal class MessageDecoder(
    private val unpacker: MessageUnpacker,
    private val sealedDescriptor: SerialDescriptor? = null,
) : AbstractDecoder() {
    constructor(buffer: ByteArray) : this(MessagePack.newDefaultUnpacker(buffer))

    private var elementIndex = 0

    override val serializersModule: SerializersModule = EmptySerializersModule()

    override fun decodeSequentially(): Boolean = true

    override fun decodeElementIndex(descriptor: SerialDescriptor): Int {
        if (elementIndex == descriptor.elementsCount) {
            return CompositeDecoder.DECODE_DONE
        } else {
            return elementIndex++
        }
    }

    override fun decodeNotNullMark(): Boolean = !unpacker.tryUnpackNil()

    override fun decodeEnum(descriptor: SerialDescriptor): Int {
        val value = decodeInt()

        for (index in 0..<descriptor.elementsCount) {
            val annotations = descriptor.getElementAnnotations(index)
            val variantValue = annotations.firstNotNullOfOrNull { if (it is Value) it.value else null } ?: index

            if (variantValue == value) {
                return index
            }
        }

        throw SerializationException("wrong value $value for ${descriptor.serialName}")
    }

    override fun decodeBoolean(): Boolean = unpacker.unpackBoolean()

    override fun decodeLong(): Long = unpacker.unpackLong()

    override fun decodeInt(): Int = unpacker.unpackInt()

    override fun decodeShort(): Short = unpacker.unpackShort()

    override fun decodeByte(): Byte = unpacker.unpackByte()

    override fun decodeString(): String {
        val descriptor = sealedDescriptor

        if (descriptor != null) {
            // We are decoding a sealed class instance. Decode the unqualified class name first,
            // then find the matching element and get the fully qualified name from it.
            val type = unpacker.getNextFormat().getValueType()
            when (type) {
                ValueType.MAP -> {
                    val size = unpacker.unpackMapHeader()
                    if (size != 1) {
                        throw SerializationException("wrong map size (expected: 1, actual: $size)")
                    }
                }
                ValueType.STRING -> {}
                else -> throw SerializationException("wrong type (expected: MAP or STRING, actual: $type)")
            }

            val simpleName = unpacker.unpackString()
            val qualifiedName = descriptor.elementDescriptors.find { unqualify(it.serialName) == simpleName }?.serialName

            if (qualifiedName != null) {
                return qualifiedName
            } else {
                throw SerializationException("wrong subclass $simpleName for ${descriptor.serialName}")
            }
        } else {
            return unpacker.unpackString()
        }
    }

    override fun decodeInline(descriptor: SerialDescriptor): Decoder {
        return MessageDecoder(unpacker)
    }

    override fun beginStructure(descriptor: SerialDescriptor): CompositeDecoder {
        when (descriptor.kind) {
            PolymorphicKind.SEALED -> return MessageDecoder(
                unpacker,
                sealedDescriptor = descriptor.getElementDescriptor(1),
            )
            StructureKind.CLASS -> {
                val count = unpacker.unpackArrayHeader()
                if (count != descriptor.elementsCount) {
                    throw SerializationException("wrong field count for ${descriptor.serialName} (expected: ${descriptor.elementsCount}, actual: $count)")
                }
            }
            else -> {}
        }

        return MessageDecoder(unpacker)
    }

    override fun decodeCollectionSize(descriptor: SerialDescriptor): Int = when (descriptor.kind) {
        StructureKind.LIST -> unpacker.unpackArrayHeader()
        StructureKind.CLASS, StructureKind.OBJECT, StructureKind.MAP -> unpacker.unpackMapHeader()
        else -> throw SerializationException("wrong collection kind ${descriptor.kind}")
    }

    @Suppress("UNCHECKED_CAST")
    override fun <T> decodeSerializableValue(deserializer: DeserializationStrategy<T>): T {
        return if (deserializer == ByteArraySerializer()) {
            decodeByteArray() as T
        } else {
            super.decodeSerializableValue(deserializer)
        }
    }

    fun decodeByteArray(): ByteArray {
        val size = unpacker.unpackBinaryHeader()
        return unpacker.readPayload(size)
    }
}

private fun unqualify(name: String): String {
    val i = name.lastIndexOf('.')
    return if (i >= 0) name.substring(i + 1) else name
}
