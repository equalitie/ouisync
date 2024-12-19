package org.equalitie.ouisync.lib

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.nio.file.Path
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.msgpack.core.MessagePack
import org.newsclub.net.unix.AFUNIXSocket
import org.newsclub.net.unix.AFUNIXSocketAddress

/**
 * Receiver for events of type `E`
 */
class EventReceiver<E> internal constructor(
    private val client: Client,
    private val id: Long,
    private val channel: ReceiveChannel<Any?>,
) {
    /**
     * Receives next event
     */
    suspend fun receive(): E {
        @Suppress("UNCHECKED_CAST")
        return channel.receive() as E
    }

    /**
     * Unsubscribes from the events
     */
    suspend fun close() {
        channel.cancel()
        client.unsubscribe(id)
    }

    /**
     * Converts this receiver into [Flow]. The receiver is automatically
     * [close]d after the flow is collected.
     */
    fun consumeAsFlow(): Flow<E> = flow {
        try {
            while (true) {
                emit(receive())
            }
        } finally {
            close()
        }
    }
}

internal class Client private constructor(
    private val socket: Socket,
) {
    companion object {
        suspend fun connect(socketPath: String): Client {
            // TODO: retry connect

            val socket = Socket.connect(socketPath)

            return Client(socket)
        }
    }

    private val messageMatcher = MessageMatcher()
    private var receiveJob: Job? = null

    init {
        // TODO: get coroutine scope
        receiveJob = launch {
            receive()
        }
    }

    suspend fun invoke(request: Request): Any? {
        val response = messageMatcher.response()

        val stream = ByteArrayOutputStream()
        DataOutputStream(stream).writeLong(response.id)

        val packer = MessagePack.newDefaultPacker(stream)
        request.pack(packer)
        packer.close()

        socket.write(stream.toByteArray())

        return response.await()
    }

    suspend fun <E> subscribe(request: Request): EventReceiver<E> {
        TODO()
    }

    suspend fun unsubscribe(id: Long) {
        TODO()
    }

    suspend fun close() {
        socket.close()
        receiveJob.cancelAndJoin()
    }

    private suspend fun receive() {
        // val buffer = socket.read(8)
        // val

        // val stream = ByteArrayInputStream(buffer)

        // val id = try {
        //     DataInputStream(stream).readLong()
        // } catch (e: EOFException) {
        //     return
        // }

        // try {
        //     val unpacker = MessagePack.newDefaultUnpacker(stream)
        //     val message = ServerMessage.unpack(unpacker)

        //     when (message) {
        //         is Success -> handleSuccess(id, message.value)
        //         is Failure -> handleFailure(id, message.error)
        //         is Notification -> handleNotification(id, message.content)
        //     }
        // } catch (e: InvalidResponse) {
        //     handleFailure(id, e)
        // } catch (e: InvalidNotification) {
        //     // TODO: log?
        // } catch (e: Exception) {
        //     // TODO: log?
        // }
    }
}

private class MessageMatcher {
    private var nextId: Long = 0
    private val mutex = Mutex()
    private val responses: HashMap<Long, CompletableDeferred<Any?>> = HashMap()
    private val subscriptions: HashMap<Long, SendChannel<Any?>> = HashMap()


}

// Async wrapper around AFUNIXSocket
// TODO: Look into nio and non-blocking mode
private class Socket(
    private val socket: AFUNIXSocket,
    private val inputStream: InputStream,
    private val outputStream: OutputStream,
) {
    companion object {
        suspend fun connect(path: String): Socket = withContext(Dispatchers.IO) {
            val socket = AFUNIXSocket.newInstance()

            val addr = AFUNIXSocketAddress.of(Path.of(path))
            socket.connect(addr)

            val inputStream = socket.getInputStream()
            val outputStream = socket.getOutputStream()

            Socket(socket, inputStream, outputStream)
        }
    }

    suspend fun read(len: Int): ByteArray = withContext(Dispatchers.IO) {
        inputStream.readNBytes(len)
    }

    suspend fun write(buffer: ByteArray) = withContext(Dispatchers.IO) {
        outputStream.write(buffer)
    }

    suspend fun close() = withContext(Dispatchers.IO) {
        inputStream.close()
        outputStream.close()
        socket.close()
    }
}


// import kotlinx.coroutines.channels.Channel
// import kotlinx.coroutines.channels.SendChannel
// import java.io.DataInputStream
// import java.io.DataOutputStream
// import java.io.EOFException


// internal class Client {
//     var sessionHandle: Long = 0

//     private var nextMessageId: Long = 0
//     private val subscriptions: HashMap<Long, SendChannel<Any?>> = HashMap()

//     suspend fun invoke(request: Request): Any? {
//         val id = nextMessageId++

//         val stream = ByteArrayOutputStream()
//         DataOutputStream(stream).writeLong(id)
//         val packer = MessagePack.newDefaultPacker(stream)
//         request.pack(packer)
//         packer.close()
//         val message = stream.toByteArray()

//         val deferred = CompletableDeferred<Any?>()

//         mutex.withLock {
//             responses.put(id, deferred)
//         }

//         Session.bindings.session_channel_send(sessionHandle, message, message.size)

//         return deferred.await()
//     }

//     suspend fun receive(buffer: ByteArray) {
//         val stream = ByteArrayInputStream(buffer)

//         val id = try {
//             DataInputStream(stream).readLong()
//         } catch (e: EOFException) {
//             return
//         }

//         try {
//             val unpacker = MessagePack.newDefaultUnpacker(stream)
//             val message = ServerMessage.unpack(unpacker)

//             when (message) {
//                 is Success -> handleSuccess(id, message.value)
//                 is Failure -> handleFailure(id, message.error)
//                 is Notification -> handleNotification(id, message.content)
//             }
//         } catch (e: InvalidResponse) {
//             handleFailure(id, e)
//         } catch (e: InvalidNotification) {
//             // TODO: log?
//         } catch (e: Exception) {
//             // TODO: log?
//         }
//     }

//     suspend fun <E> subscribe(request: Request): EventReceiver<E> {
//         // FIXME: race condition - if a notification arrives after we call `invoke` but before we
//         // register the sender then it gets silently dropped

//         val id = invoke(request) as Long
//         val channel = Channel<Any?>(1024)

//         mutex.withLock {
//             subscriptions.put(id, channel)
//         }

//         return EventReceiver(this, id, channel)
//     }

//     suspend fun unsubscribe(id: Long) {
//         mutex.withLock {
//             subscriptions.remove(id)
//         }

//         invoke(Unsubscribe(id))
//     }

//     private suspend fun handleSuccess(id: Long, content: Any?) {
//         mutex.withLock {
//             responses.remove(id)?.complete(content)
//         }
//     }

//     private suspend fun handleNotification(id: Long, content: Any?) {
//         mutex.withLock {
//             subscriptions.get(id)?.send(content)
//         }
//     }

//     private suspend fun handleFailure(id: Long, error: Exception) {
//         mutex.withLock {
//             responses.remove(id)?.completeExceptionally(error)
//         }
//     }
// }

// private data class ServerEnvelope(val id: Long, val content: ServerMessage)



// Adapters to add suspend API to AsynchronousSocketChannel
// (Stolen from https://github.com/Kotlin/kotlinx.coroutines/blob/87eaba8a287285d4c47f84c91df7671fcb58271f/integration/kotlinx-coroutines-nio/src/Nio.kt)

// private suspend fun AsynchronousSocketChannel.asyncConnect(
//     addr: SocketAddress
// ) = suspendCoroutine<Unit> { cont ->
//     connect(addr, cont, object : CompletionHandler<Void?, Continuation<Unit>> {
//         override fun completed(result: Void?, cont: Continuation<Unit>) {
//             cont.resume(Unit)
//         }

//         override fun failed(ex: Throwable, cont: Continuation<Unit>) {
//             cont.resumeWithException(ex)
//         }
//     })
// }

// private suspend fun AsynchronousSocketChannel.asyncRead(
//     buffer: ByteBuffer,
//     timeout: Long = 0L,
//     timeUnit: TimeUnit = TimeUnit.MILLISECONDS
// ) = suspendCancellableCoroutine<Int> { cont ->
//     read(buffer, timeout, timeUnit, cont, AsyncIOHandler())
//     closeOnCancel(cont)
// }

// private suspend fun AsynchronousSocketChannel.asyncWrite(
//     buffer: ByteBuffer,
//     timeout: Long = 0L,
//     timeUnit: TimeUnit = TimeUnit.MILLISECONDS
// ) = suspendCancellableCoroutine<Int> { cont ->
//     write(buffer, timeout, timeUnit, cont, AsyncIOHandler())
//     closeOnCancel(cont)
// }

