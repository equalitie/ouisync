package org.equalitie.ouisync.lib

import kotlinx.coroutines.CancellableContinuation
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.SendChannel
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.channelFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.serialization.json.Json
import org.msgpack.core.MessagePack
import java.io.File
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.SocketAddress
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.channels.AsynchronousCloseException
import java.nio.channels.AsynchronousSocketChannel
import java.nio.channels.CompletionHandler
import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlin.math.max

internal class Client private constructor(private val socket: AsynchronousSocket) {
    companion object {
        @OptIn(kotlin.ExperimentalStdlibApi::class)
        suspend fun connect(configPath: String): Client {
            val portRaw = File("$configPath/local_control_port.conf").readText()
            val port = Json.decodeFromString<Int>(portRaw)

            val authKeyRaw = File("$configPath/local_control_auth_key.conf").readText()
            val authKey = Json.decodeFromString<String>(authKeyRaw).hexToByteArray()

            // TODO: retry connect

            val socket = AsynchronousSocket.connect(InetSocketAddress(InetAddress.getLoopbackAddress(), port))

            authenticate(socket, authKey)

            return Client(socket)
        }
    }

    private val messageMatcher = MessageMatcher()
    private val coroutineScope = CoroutineScope(Dispatchers.Default)

    init {
        coroutineScope.launch {
            receive()
        }
    }

    suspend fun invoke(request: Request): Any {
        val id = messageMatcher.nextId()
        val deferred = CompletableDeferred<Response>()
        messageMatcher.register(id, deferred)

        send(id, request)

        val response = deferred.await()

        when (response) {
            is Success -> return response.value
            is Failure -> throw response.error
        }
    }

    fun subscribe(request: Request): Flow<Any> = channelFlow {
        val id = messageMatcher.nextId()
        messageMatcher.register(id, channel)

        try {
            send(id, request)
            awaitClose()
        } finally {
            messageMatcher.deregister(id)
            invoke(Unsubscribe(id))
        }
    }
        .buffer(onBufferOverflow = BufferOverflow.DROP_OLDEST)
        .map {
            when (it) {
                is Success -> it.value
                is Failure -> throw it.error
            }
        }

    suspend fun close() {
        coroutineScope.cancel()
        socket.close()
    }

    private suspend fun send(id: Long, request: Request) {
        // Message format:
        //
        // | length  | message_id | payload            |
        // +---------+------------+--------------------+
        // | u32, be | u64, be    | `length` - 8 bytes |

        val payload = MessagePack.newDefaultBufferPacker().let {
            request.pack(it)
            it.toByteArray()
        }

        val buffer = ByteBuffer.allocate(HEADER_SIZE + payload.size)
        buffer.order(ByteOrder.BIG_ENDIAN)
        buffer.putInt(payload.size + Long.SIZE_BYTES)
        buffer.putLong(id)
        buffer.put(payload)
        buffer.flip()

        socket.writeAll(buffer)
    }

    private suspend fun receive() {
        var buffer = ByteBuffer.allocate(HEADER_SIZE)
        buffer.order(ByteOrder.BIG_ENDIAN)

        while (true) {
            buffer.limit(HEADER_SIZE)
            buffer.rewind()
            socket.readExact(buffer)
            buffer.flip()

            if (buffer.remaining() < HEADER_SIZE) {
                messageMatcher.cancel()
                break
            }

            val size = buffer.getInt() - Long.SIZE_BYTES
            val id = buffer.getLong()

            if (size > buffer.capacity()) {
                buffer = ByteBuffer.allocate(max(2 * buffer.capacity(), size))
                buffer.order(ByteOrder.BIG_ENDIAN)
            }

            buffer.limit(size)
            buffer.rewind()
            socket.readExact(buffer)
            buffer.flip()

            val completer = messageMatcher.completer(id)

            if (completer == null) {
                // unsolicited response
                continue
            }

            try {
                val unpacker = MessagePack.newDefaultUnpacker(buffer)
                val response = Response.unpack(unpacker)

                completer.complete(response)
            } catch (e: Error) {
                completer.complete(Failure(e))
            } catch (e: Exception) {
                completer.complete(Failure(Error.InvalidData("invalid response: $e")))
            }
        }
    }
}

private class MessageMatcher {
    private var nextId: Long = 0
    private val oneshots: HashMap<Long, CompletableDeferred<Response>> = HashMap()
    private val channels: HashMap<Long, SendChannel<Response>> = HashMap()

    @Synchronized
    fun nextId(): Long = nextId++

    @Synchronized
    fun register(id: Long, deferred: CompletableDeferred<Response>) {
        oneshots.put(id, deferred)
    }

    @Synchronized
    fun register(id: Long, channel: SendChannel<Response>) {
        channels.put(id, channel)
    }

    // This deregisters only channels because deferreds are unregistered automatically on
    // completion.
    @Synchronized
    fun deregister(id: Long) {
        channels.remove(id)
    }

    @Synchronized
    fun completer(id: Long): Completer? {
        val deferred = oneshots.remove(id)
        if (deferred != null) {
            return Completer.Oneshot(deferred)
        }

        val channel = channels.get(id)
        if (channel != null) {
            return Completer.Channel(channel)
        }

        return null
    }

    @Synchronized
    fun cancel() {
        for (deferred in oneshots.values) {
            deferred.cancel()
        }
        oneshots.clear()

        for (channel in channels.values) {
            channel.close()
        }
        channels.clear()
    }
}

private sealed class Completer {
    class Oneshot(val deferred: CompletableDeferred<Response>) : Completer() {
        override suspend fun complete(value: Response) {
            deferred.complete(value)
        }
    }

    class Channel(val channel: SendChannel<Response>) : Completer() {
        override suspend fun complete(value: Response) = channel.send(value)
    }

    abstract suspend fun complete(value: Response)
}

private const val HEADER_SIZE = Int.SIZE_BYTES + Long.SIZE_BYTES

private const val CHALLENGE_SIZE = 256
private const val PROOF_SIZE = 32

private suspend fun authenticate(socket: AsynchronousSocket, authKey: ByteArray) {
    val random = SecureRandom()

    val hmacAlgo = "HmacSHA256"
    val hmacKey = SecretKeySpec(authKey, hmacAlgo)
    val hmac = Mac.getInstance(hmacAlgo).apply { init(hmacKey) }

    val buffer = ByteBuffer.allocate(CHALLENGE_SIZE + PROOF_SIZE)

    val clientChallenge = ByteArray(CHALLENGE_SIZE)
    random.nextBytes(clientChallenge)

    buffer.put(clientChallenge)
    buffer.flip()
    socket.writeAll(buffer)

    buffer.limit(CHALLENGE_SIZE + PROOF_SIZE)
    buffer.rewind()
    socket.readExact(buffer)
    buffer.flip()

    val serverProof = ByteArray(PROOF_SIZE)
    buffer.get(serverProof)

    if (!MessageDigest.isEqual(serverProof, hmac.doFinal(clientChallenge))) {
        throw Error.PermissionDenied()
    }

    val serverChallenge = ByteArray(CHALLENGE_SIZE)
    buffer.get(serverChallenge)

    hmac.init(hmacKey)
    val clientProof = hmac.doFinal(serverChallenge)

    buffer.limit(CHALLENGE_SIZE)
    buffer.rewind()
    buffer.put(clientProof)
    buffer.flip()

    socket.writeAll(buffer)
}

// Wrapper around AsynchronousSocketChannel which provides a convenient, coroutine based API.
private class AsynchronousSocket(private val channel: AsynchronousSocketChannel) {
    companion object {
        suspend fun connect(addr: SocketAddress): AsynchronousSocket {
            val channel = AsynchronousSocketChannel.open()

            suspendCancellableCoroutine<Unit> { cont ->
                channel.connect(addr, cont, ConnectHandler)
            }

            return AsynchronousSocket(channel)
        }
    }

    suspend fun read(buffer: ByteBuffer) = suspendCancellableCoroutine<Int> { cont ->
        channel.read(buffer, cont, IOHandler())
    }

    suspend fun readExact(buffer: ByteBuffer): Int {
        var total = 0

        while (buffer.hasRemaining()) {
            val n = read(buffer)

            if (n == 0) {
                break
            } else {
                total += n
            }
        }

        return total
    }

    suspend fun write(buffer: ByteBuffer) = suspendCancellableCoroutine<Int> { cont ->
        channel.write(buffer, cont, IOHandler())
    }

    suspend fun writeAll(buffer: ByteBuffer) {
        while (buffer.hasRemaining()) {
            write(buffer)
        }
    }

    fun close() {
        channel.close()
    }
}

object ConnectHandler : CompletionHandler<Void?, CancellableContinuation<Unit>> {
    override fun completed(result: Void?, cont: CancellableContinuation<Unit>) {
        cont.resume(Unit)
    }

    override fun failed(ex: Throwable, cont: CancellableContinuation<Unit>) {
        if (ex is AsynchronousCloseException && cont.isCancelled) return
        cont.resumeWithException(ex)
    }
}

class IOHandler<T> : CompletionHandler<T, CancellableContinuation<T>> {
    override fun completed(result: T, cont: CancellableContinuation<T>) {
        cont.resume(result)
    }

    override fun failed(ex: Throwable, cont: CancellableContinuation<T>) {
        // just return if already cancelled and got an expected exception for that case
        if (ex is AsynchronousCloseException && cont.isCancelled) return
        cont.resumeWithException(ex)
    }
}
