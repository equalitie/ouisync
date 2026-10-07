@file:UseSerializers(OuisyncExceptionSerializer::class, InetAddressSerializer::class)

package org.equalitie.ouisync.session

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.SendChannel
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.channelFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.UseSerializers
import kotlinx.serialization.json.Json
import java.io.EOFException
import java.io.File
import java.io.IOException
import java.net.InetSocketAddress
import java.net.URI
import java.net.URLDecoder
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.charset.StandardCharsets
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.concurrent.TimeoutException
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec
import kotlin.math.max
import kotlin.time.Clock
import kotlin.time.Duration
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds

internal class Client private constructor(private val socket: Socket) {
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
            val serviceAddress = readServiceAddress(configPath)

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
                    val socket = Socket.connect(serviceAddress.socketAddress)

                    serviceAddress.authKey?.let { authKey -> authenticate(socket, authKey) }

                    return Client(socket)
                } catch (e: IOException) {
                    error = e
                }

                wait = wait * 2
                wait = if (wait < maxWait) wait else maxWait

                delay(wait)
            }
        }
    }

    private val messageMatcher = MessageMatcher()
    private val receiveScope = CoroutineScope(Dispatchers.Default)
    private val writeMutex = Mutex()

    init {
        receiveScope.launch { receive() }
    }

    suspend fun invoke(request: Request): Any {
        val id = messageMatcher.nextId()
        val deferred = CompletableDeferred<ResponseResult>()
        messageMatcher.register(id, deferred)

        val response =
            try {
                send(id, request)
                deferred.await()
            } catch (e: CancellationException) {
                try {
                    withContext(NonCancellable) { invoke(Request.Cancel(MessageId(id))) }
                } catch (cancelException: Exception) {
                    e.addSuppressed(cancelException)
                }

                throw e
            }

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

            // Use `NonCancellable` because this is typically reached when the flow collection is
            // cancelled and the request would otherwise not be sent at all. The cancel is best
            // effort, so ignore any errors (e.g., the connection being already closed) to not
            // override the original outcome of the flow.
            try {
                withContext(NonCancellable) { invoke(Request.Cancel(MessageId(id))) }
            } catch (_: Exception) {}
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
        receiveScope.cancel()
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

        writeMutex.withLock { socket.writeAll(buffer) }
    }

    private suspend fun receive() {
        try {
            var buffer = ByteBuffer.allocate(HEADER_SIZE)
            buffer.order(ByteOrder.BIG_ENDIAN)

            while (true) {
                buffer.limit(HEADER_SIZE)
                buffer.rewind()
                socket.readExact(buffer)
                buffer.flip()

                if (buffer.remaining() < HEADER_SIZE) {
                    throw EOFException()
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
                    throw EOFException()
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
        } catch (e: Exception) {
            socket.close()
            messageMatcher.close(e)
        }
    }
}

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
    fun close(cause: Exception? = null) {
        closed = true

        for (deferred in oneshots.values) {
            if (cause != null) {
                deferred.completeExceptionally(cause)
            } else {
                deferred.cancel()
            }
        }
        oneshots.clear()

        for (channel in channels.values) {
            channel.close(cause)
        }
        channels.clear()
    }
}

private sealed class Completer {
    class Oneshot(val deferred: CompletableDeferred<ResponseResult>) : Completer() {
        override fun complete(value: ResponseResult) {
            deferred.complete(value)
        }
    }

    class Channel(val channel: SendChannel<ResponseResult>) : Completer() {
        override fun complete(value: ResponseResult) {
            if (value is ResponseResult.Success && value.value is Response.None) {
                channel.close()
            } else {
                // We can safely use `trySend` because the channel uses
                // `BufferOverflow.DROP_OLDEST`.
                channel.trySend(value)
            }
        }
    }

    abstract fun complete(value: ResponseResult)
}

private suspend fun Socket.readExact(buffer: ByteBuffer): Int {
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

private suspend fun Socket.writeAll(buffer: ByteBuffer) {
    while (buffer.hasRemaining()) {
        write(buffer)
    }
}

private data class ServiceAddress(val socketAddress: SocketAddress, val authKey: ByteArray?)

private fun readServiceAddress(configDir: String): ServiceAddress {
    val unixSocket = File(configDir, "local_endpoint.sock")
    if (unixSocket.exists()) {
        return ServiceAddress(SocketAddress.Unix(unixSocket.path), null)
    }

    val tcpConf = File(configDir, "local_endpoint.conf")
    val uri = URI(Json.decodeFromString<String>(tcpConf.readText()))

    if (uri.scheme != "tcp") {
        throw IllegalArgumentException("invalid service address: $uri - unuported scheme")
    }

    val authKey =
        uri.rawQuery
            ?.split("&")
            ?.map { it.split("=", limit = 2) }
            ?.firstOrNull { URLDecoder.decode(it[0], StandardCharsets.UTF_8) == "auth_key" }
            ?.let {
                if (it.size > 1) {
                    URLDecoder.decode(it[1], StandardCharsets.UTF_8)
                } else {
                    null
                }
            }
            ?.let { it.hexToByteArray() }

    if (authKey == null) {
        throw IllegalArgumentException("invalid service address: $uri - missing or invalid auth_key")
    }

    return ServiceAddress(SocketAddress.Tcp(InetSocketAddress(uri.host, uri.port)), authKey)
}

private const val HEADER_SIZE = Int.SIZE_BYTES + Long.SIZE_BYTES

private const val CHALLENGE_SIZE = 256
private const val PROOF_SIZE = 32

private suspend fun authenticate(socket: Socket, authKey: ByteArray) {
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
