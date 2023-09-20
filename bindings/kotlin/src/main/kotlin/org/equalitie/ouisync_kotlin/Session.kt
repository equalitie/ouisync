package org.equalitie.ouisync_kotlin

import com.sun.jna.Pointer
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.EOFException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.msgpack.core.MessagePack

import org.equalitie.ouisync_kotlin.NetworkInitRequest
import org.equalitie.ouisync_kotlin.Request
import org.equalitie.ouisync_kotlin.Response
import org.equalitie.ouisync_kotlin.ServerMessage

class Session private constructor(val handle: Long, private val client: Client) {
    companion object {
        internal val bindings = Bindings.INSTANCE

        fun create(configsPath: String, logPath: String): Session {
            val client = Client()

            val callback = object : Callback {
                override fun invoke(context: Pointer?, msg_ptr: Pointer, msg_len: Long) {
                    val buffer = msg_ptr.getByteArray(0, msg_len.toInt())

                    runBlocking {
                        client.receive(buffer)
                    }
                }
            }

            val result = bindings.session_create(
                configsPath,
                logPath,
                null,
                callback
            )

            if (result.error_code == ErrorCode.OK) {
                return Session(result.handle, client)
            } else {
                val message = result.error_message?.getString(0) ?: "unknown error"
                bindings.free_string(result.error_message)

                throw Error(result.error_code, message)
            }
        }
    }

    init {
        client.sessionHandle = handle
    }

    fun dispose() {
        bindings.session_destroy(handle)
    }

    suspend fun initNetwork(
        defaultPortForwardingEnabled: Boolean,
        defaultLocalDiscoveryEnabled: Boolean,
    ) {
        val response = client.invoke(NetworkInitRequest(
            defaultLocalDiscoveryEnabled,
            defaultPortForwardingEnabled
        ))

        assert(response == null)
    }
}

private class Client {
    internal var sessionHandle: Long = 0

    val mutex = Mutex()
    var nextMessageId: Long = 0
    val responses: HashMap<Long, CompletableDeferred<Any?>> = HashMap()

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
                is Response -> handleResponse(id, message.content)
                is Notification -> handleNotification(id, message.content)
            }
        } catch (e: Exception) {
            handleInvalidMessage(id)
        }
    }

    private suspend fun handleResponse(id: Long, content: Any?) {
        mutex.withLock {
            responses.remove(id)?.complete(content)
        }
    }

    private suspend fun handleNotification(id: Long, content: Any?) {
        // ...
    }

    private suspend fun handleInvalidMessage(id: Long) {
        // ...
    }

    private fun getMessageId(): Long {
        val id = nextMessageId
        nextMessageId += 1
        return id
    }
}

private data class ServerEnvelope(val id: Long, val content: ServerMessage)



