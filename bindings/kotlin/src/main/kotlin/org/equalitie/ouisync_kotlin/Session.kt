package org.equalitie.ouisync_kotlin

import com.sun.jna.Pointer
import kotlinx.coroutines.runBlocking

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

    suspend fun bindNetwork(
        quicV4: String? = null,
        quicV6: String? = null,
        tcpV4: String? = null,
        tcpV6: String? = null,
    ) {
        val response = client.invoke(NetworkBindRequest(quicV4, quicV6, tcpV4, tcpV6))
        assert(response == null)
    }

    suspend fun quicListenerLocalAddrV4(): String? =
        client.invoke(NetworkQuicListenerLocalAddrV4Request()) as String?

    suspend fun quicListenerLocalAddrV6(): String? =
        client.invoke(NetworkQuicListenerLocalAddrV6Request()) as String?

    suspend fun tcpListenerLocalAddrV4(): String? =
        client.invoke(NetworkTcpListenerLocalAddrV4Request()) as String?

    suspend fun tcpListenerLocalAddrV6(): String? =
        client.invoke(NetworkTcpListenerLocalAddrV6Request()) as String?

    suspend fun addUserProvidedPeer(addr: String) {
        val response = client.invoke(NetworkAddUserProvidedPeerRequest(addr))
        assert(response == null)
    }

    suspend fun removeUserProvidedPeer(addr: String) {
        val response = client.invoke(NetworkRemoveUserProvidedPeerRequest(addr))
        assert(response == null)
    }
}

