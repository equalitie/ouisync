package org.equalitie.ouisync_kotlin

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.EOFException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.msgpack.core.MessagePack

internal class Client {
    var sessionHandle: Long = 0

    private val mutex = Mutex()
    private var nextMessageId: Long = 0
    private val responses: HashMap<Long, CompletableDeferred<Any?>> = HashMap()

    suspend fun invoke(request: Request): Any? {
        val id = getMessageId()

        val stream = ByteArrayOutputStream()
        DataOutputStream(stream).writeLong(id)
        val packer = MessagePack.newDefaultPacker(stream)
        request.pack(packer)
        packer.close()
        val message = stream.toByteArray()

        val deferred = CompletableDeferred<Any?>()

        mutex.withLock {
            responses.put(id, deferred)
        }

        Session.bindings.session_channel_send(sessionHandle, message, message.size)

        return deferred.await()
    }

    suspend fun receive(buffer: ByteArray) {
        val stream = ByteArrayInputStream(buffer)

        val id = try {
            DataInputStream(stream).readLong()
        } catch (e: EOFException) {
            return
        }

        try {
            val unpacker = MessagePack.newDefaultUnpacker(stream)
            val message = ServerMessage.unpack(unpacker)

            when (message) {
                is Success -> handleSuccess(id, message.value)
                is Failure -> handleFailure(id, message.error)
                is Notification -> handleNotification(id, message.content)
            }
        } catch (e: Exception) {
            handleFailure(id, e)
        }
    }

    private suspend fun handleSuccess(id: Long, content: Any?) {
        mutex.withLock {
            responses.remove(id)?.complete(content)
        }
    }

    private suspend fun handleNotification(id: Long, content: Any?) {
        // ...
    }

    private suspend fun handleFailure(id: Long, error: Exception) {
        mutex.withLock {
            responses.remove(id)?.completeExceptionally(error)
        }
    }

    private fun getMessageId(): Long {
        val id = nextMessageId
        nextMessageId += 1
        return id
    }
}

private data class ServerEnvelope(val id: Long, val content: ServerMessage)



