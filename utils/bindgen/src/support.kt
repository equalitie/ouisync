import kotlin.time.Duration
import kotlinx.datetime.Instant
import org.msgpack.core.MessagePacker
import org.msgpack.core.MessageUnpacker
import org.msgpack.value.ValueType


// TODO: change base class to OuisyncException.InvalidData
class DecodeError : RuntimeException {
    public constructor() : super("decode error")
}

private fun Byte.encode(p: MessagePacker) = p.packByte(this)
private fun Byte.Companion.decode(u: MessageUnpacker): Byte = u.unpackByte()

private fun Short.encode(p: MessagePacker) = p.packShort(this)
private fun Short.Companion.decode(u: MessageUnpacker): Short = u.unpackShort()

private fun Int.encode(p: MessagePacker) = p.packInt(this)
private fun Int.Companion.decode(u: MessageUnpacker): Int = u.unpackInt()

private fun Long.encode(p: MessagePacker) = p.packLong(this)
private fun Long.Companion.decode(u: MessageUnpacker): Long = u.unpackLong()

private fun Boolean.encode(p: MessagePacker) = p.packBoolean(this)
private fun Boolean.Companion.decode(u: MessageUnpacker): Boolean = u.unpackBoolean()

private fun String.encode(p: MessagePacker) = p.packString(this)
private fun String.Companion.decode(u: MessageUnpacker): String = u.unpackString()

private fun Duration.encode(p: MessagePacker) = p.packLong(inWholeMilliseconds)
private fun Duration.Companion.decode(u: MessageUnpacker): Duration = u.unpackLong().milliseconds

private fun Instant.encode(p: MessagePacker) = p.packLong(toEpochMilliseconds())
private fun Instant.Companion.decode(u: MessageUnpacker): Instant = fromEpochMilliseconds(u.unpackLong())