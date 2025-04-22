@file:OptIn(kotlin.ExperimentalStdlibApi::class)

package org.equalitie.ouisync.lib

import kotlinx.serialization.KSerializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.serializer
import org.junit.Assert.assertEquals
import org.junit.Test
import org.msgpack.core.MessagePack
import org.msgpack.core.MessagePacker

class SerializationTest {
    @Test
    fun encodeDecodeEnum() {
        encodeDecode(TestEnum.A) { packInt(0) }
        encodeDecode(TestEnum.B) { packInt(1) }
        encodeDecode(TestEnum.C) { packInt(2) }
    }

    @Test
    fun encodeDecodeAnnotatedEnum() {
        encodeDecode(TestAnnotatedEnum.Two) { packInt(2) }
        encodeDecode(TestAnnotatedEnum.Four) { packInt(4) }
    }

    @Test
    fun encodeDecodeClass() {
        encodeDecode(TestClass("alice", 1234)) {
            packArrayHeader(2)
            packString("alice")
            packLong(1234)
        }
    }

    @Test
    fun encodeDecodeSealed() {
        encodeDecode(TestSealed.A(true) as TestSealed) {
            packMapHeader(1)
            packString("A")
            packArrayHeader(1)
            packBoolean(true)
        }

        encodeDecode(TestSealed.B("hello", 42) as TestSealed) {
            packMapHeader(1)
            packString("B")
            packArrayHeader(2)
            packString("hello")
            packLong(42)
        }

        encodeDecode(TestSealed.C as TestSealed) {
            packString("C")
        }
    }

    @Test
    fun encodeDecodeNullable() {
        encodeDecode(TestNullable("foo")) {
            packArrayHeader(1)
            packString("foo")
        }

        encodeDecode(TestNullable(null)) {
            packArrayHeader(1)
            packNil()
        }
    }

    @Test
    fun encodeDecodeList() {
        encodeDecode(TestList(emptyList())) {
            packArrayHeader(1)
            packArrayHeader(0)
        }

        encodeDecode(TestList(listOf("foo", "bar", "baz"))) {
            packArrayHeader(1)
            packArrayHeader(3)
            packString("foo")
            packString("bar")
            packString("baz")
        }
    }

    @Test
    fun encodeDecodeMap() {
        encodeDecode(TestMap(emptyMap())) {
            packArrayHeader(1)
            packMapHeader(0)
        }

        encodeDecode(TestMap(mapOf("a" to 1, "b" to 2, "c" to 3))) {
            packArrayHeader(1)
            packMapHeader(3)
            packString("a")
            packInt(1)
            packString("b")
            packInt(2)
            packString("c")
            packInt(3)
        }
    }

    @Test
    fun encodeDecodeInlineClass() {
        encodeDecode(TestInline(1)) {
            packInt(1)
        }
    }

    @Test
    fun encodeDecodeSealedWithInlineClass() {
        encodeDecode(TestSealedWithInline.A(5) as TestSealedWithInline) {
            packMapHeader(1)
            packString("A")
            packShort(5)
        }

        encodeDecode(TestSealedWithInline.B("foo") as TestSealedWithInline) {
            packMapHeader(1)
            packString("B")
            packString("foo")
        }
    }

    @Test
    fun encodeDecodeBytes() {
        encodeDecode(TestBytes("deadc0de".hexToByteArray())) {
            packArrayHeader(1)
            packBinaryHeader(4)
            addPayload("deadc0de".hexToByteArray())
        }
    }

    // Test that encoding `value` using `serializer` produces the same output as running `expected`
    // and vice-versa.
    private fun <T> encodeDecode(serializer: KSerializer<T>, value: T, expected: MessagePacker.() -> Unit) {
        val actualEncoded = encode(serializer, value)
        val expectedEncoded = MessagePack.newDefaultBufferPacker().apply(expected).toByteArray()
        assertEquals(value.toString(), expectedEncoded.toHexString(), actualEncoded.toHexString())

        val actualDecoded = decode(serializer, expectedEncoded)
        assertEquals(value, actualDecoded)
    }

    // Convenient overload of `encodeDecode` that infers the serializer from the value type.
    private inline fun <reified T> encodeDecode(value: T, noinline expected: MessagePacker.() -> Unit) =
        encodeDecode(serializer(), value, expected)
}

@Serializable
private enum class TestEnum {
    A,
    B,
    C,
}

@Serializable
private enum class TestAnnotatedEnum {
    @Value(2)
    Two,

    @Value(4)
    Four,
}

@Serializable
private data class TestClass(val name: String, val code: Long)

@Serializable
private sealed interface TestSealed {
    @Serializable
    data class A(val value: Boolean) : TestSealed

    @Serializable
    data class B(val key: String, val value: Long) : TestSealed

    @Serializable
    object C : TestSealed
}

@Serializable
private data class TestNullable(val value: String?)

@Serializable
private data class TestList(val values: List<String>)

@Serializable
private data class TestMap(val values: Map<String, Int>)

@Serializable
@JvmInline
private value class TestInline(val value: Int)

@Serializable
private sealed interface TestSealedWithInline {
    @Serializable
    @JvmInline
    value class A(val value: Short) : TestSealedWithInline

    @Serializable
    @JvmInline
    value class B(val value: String) : TestSealedWithInline
}

@Serializable
private data class TestBytes(val bytes: ByteArray) {
    override fun equals(other: Any?): Boolean = other is TestBytes && bytes.contentEquals(other.bytes)
}
