@file:UseSerializers(OuisyncExceptionSerializer::class)

package org.equalitie.ouisync.session

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
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.channelFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.UseSerializers
import kotlinx.serialization.json.Json
import java.io.EOFException
import java.io.File
import java.net.ConnectException
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
import java.util.concurrent.TimeoutException
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlin.math.max
import kotlin.time.Clock
import kotlin.time.Duration
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds

internal class Client private constructor(private val socket: AsynchronousSocket) {
    companion object {
        @OptIn(
            kotlin.ExperimentalStdlibApi::class,
            kotlin.time.ExperimentalTime::class,
        )
        suspend fun connect(
            configPath: String,
            timeout: Duration? = null,
            minWait: Duration = 50.milliseconds,
            maxWait: Duration = 1.seconds,
        ): Client {
            val endpointRaw = File("$configPath/local_endpoint.conf").readText()
            val endpoint = Json.decodeFromString<LocalEndpoint>(endpointRaw)
            val port = endpoint.port
            val authKey = endpoint.authKey.hexToByteArray()

            val addr = InetSocketAddress(InetAddress.getByAddress(byteArrayOf(127, 0, 0, 1)), port)

            var start = Clock.System.now()
            var wait = minWait
            var error: Exception? = null

            while (true) {
                if (timeout != null) {
                    if (Clock.System.now() - start >= timeout) {
                        throw error ?: TimeoutException()
                    }
                }

                try {
                    val socket =
                        withContext(Dispatchers.IO) {
                            AsynchronousSocket.connect(addr).also { socket -> authenticate(socket, authKey) }
                        }

                    return Client(socket)
                } catch (e: ConnectException) {
                    error = e
                    wait = wait * 2
                    wait = if (wait < maxWait) wait else maxWait

                    delay(wait)
                }
            }
        }
    }

    private val messageMatcher = MessageMatcher()
    private val coroutineScope = CoroutineScope(Dispatchers.IO)

    init {
        coroutineScope.launch { receive() }
    }

    suspend fun invoke(request: Request): Any {
        val id = messageMatcher.nextId()
        val deferred = CompletableDeferred<ResponseResult>()
        messageMatcher.register(id, deferred)

        send(id, request)

        val response = deferred.await()

        when (response) {
            is ResponseResult.Success -> return response.value
            is ResponseResult.Failure -> throw response.error
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
            invoke(Request.SessionUnsubscribe(MessageId(id)))
        }
    }
        .buffer(onBufferOverflow = BufferOverflow.DROP_OLDEST)
        .map {
            when (it) {
                is ResponseResult.Success -> it.value
                is ResponseResult.Failure -> throw it.error
            }
        }

    suspend fun close() {
        coroutineScope.cancel()
        socket.close()
        messageMatcher.close()
    }

    private suspend fun send(id: Long, request: Request) {
        // Message format:
        //
        // | length  | message_id | payload            |
        // +---------+------------+--------------------+
        // | u32, be | u64, be    | `length` - 8 bytes |

        val payload = encode(request)

        val buffer = ByteBuffer.allocate(HEADER_SIZE + payload.size)
        buffer.order(ByteOrder.BIG_ENDIAN)
        buffer.putInt(payload.size + Long.SIZE_BYTES)
        buffer.putLong(id)
        buffer.put(payload)
        buffer.flip()

        withContext(Dispatchers.IO) { socket.writeAll(buffer) }
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

            if (buffer.remaining() < size) {
                break
            }

            val completer = messageMatcher.completer(id)

            if (completer == null) {
                // unsolicited response
                continue
            }

            try {
                val response: ResponseResult = decode(buffer.array())

                completer.complete(response)
            } catch (e: OuisyncException) {
                completer.complete(ResponseResult.Failure(e))
            } catch (e: Exception) {
                completer.complete(
                    ResponseResult.Failure(OuisyncException.InvalidData("invalid response: $e")),
                )
            }
        }

        messageMatcher.close()
    }
}

@Serializable
private data class LocalEndpoint(val port: Int, @SerialName("auth_key") val authKey: String)

@Serializable
private sealed interface ResponseResult {
    @Serializable @JvmInline
    value class Success(val value: Response) : ResponseResult

    @Serializable @JvmInline
    value class Failure(val error: OuisyncException) : ResponseResult
}

private class MessageMatcher {
    private var nextId: Long = 0
    private val oneshots: HashMap<Long, CompletableDeferred<ResponseResult>> = HashMap()
    private val channels: HashMap<Long, SendChannel<ResponseResult>> = HashMap()
    private var closed = false

    @Synchronized fun nextId(): Long = nextId++

    @Synchronized
    fun register(id: Long, deferred: CompletableDeferred<ResponseResult>) {
        if (!closed) {
            oneshots.put(id, deferred)
        } else {
            deferred.completeExceptionally(EOFException())
        }
    }

    @Synchronized
    fun register(id: Long, channel: SendChannel<ResponseResult>) {
        if (!closed) {
            channels.put(id, channel)
        } else {
            channel.close(EOFException())
        }
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
    fun close() {
        closed = true

        for (deferred in oneshots.values) {
            deferred.completeExceptionally(EOFException())
        }
        oneshots.clear()

        for (channel in channels.values) {
            channel.close(EOFException())
        }
        channels.clear()
    }
}

private sealed class Completer {
    class Oneshot(val deferred: CompletableDeferred<ResponseResult>) : Completer() {
        override suspend fun complete(value: ResponseResult) {
            deferred.complete(value)
        }
    }

    class Channel(val channel: SendChannel<ResponseResult>) : Completer() {
        override suspend fun complete(value: ResponseResult) = channel.send(value)
    }

    abstract suspend fun complete(value: ResponseResult)
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
        throw OuisyncException.PermissionDenied()
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

            suspendCancellableCoroutine<Unit> { cont -> channel.connect(addr, cont, ConnectHandler) }

            return AsynchronousSocket(channel)
        }
    }

    // Prevents `WritePendingException` on concurrent writes
    // TODO: Do we also need readMutex?
    private val writeMutex = Mutex()

    suspend fun read(buffer: ByteBuffer) = suspendCancellableCoroutine<Int> { cont -> channel.read(buffer, cont, IOHandler()) }

    suspend fun readExact(buffer: ByteBuffer): Int {
        var total = 0

        while (buffer.hasRemaining()) {
            val n = read(buffer)

            if (n <= 0) {
                break
            } else {
                total += n
            }
        }

        return total
    }

    suspend fun write(buffer: ByteBuffer) = writeMutex.withLock { writeUnlocked(buffer) }

    suspend fun writeAll(buffer: ByteBuffer) = writeMutex.withLock {
        while (buffer.hasRemaining()) {
            writeUnlocked(buffer)
        }
    }

    private suspend fun writeUnlocked(buffer: ByteBuffer) = suspendCancellableCoroutine<Int> { cont -> channel.write(buffer, cont, IOHandler()) }

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
