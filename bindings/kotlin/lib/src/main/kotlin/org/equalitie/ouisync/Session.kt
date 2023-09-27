package org.equalitie.ouisync

import com.sun.jna.Pointer
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.runBlocking
import java.io.Closeable

class Session private constructor(private val handle: Long, internal val client: Client) : Closeable {
    companion object {
        internal val bindings = Bindings.INSTANCE

        fun create(configsPath: String, logPath: String? = null): Session {
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
                callback,
            )

            val errorCode = ErrorCode.decode(result.error_code)

            if (errorCode == ErrorCode.OK) {
                return Session(result.handle, client)
            } else {
                val message = result.error_message?.getString(0) ?: "unknown error"
                bindings.free_string(result.error_message)

                throw Error(errorCode, message)
            }
        }
    }

    init {
        client.sessionHandle = handle
    }

    override fun close() {
        bindings.session_close(handle)
    }

    /**
     * Initializes the network according to the stored configuration.
     */
    suspend fun initNetwork(
        defaultPortForwardingEnabled: Boolean,
        defaultLocalDiscoveryEnabled: Boolean,
    ) {
        val response = client.invoke(
            NetworkInit(
                defaultLocalDiscoveryEnabled,
                defaultPortForwardingEnabled,
            ),
        )

        assert(response == null)
    }

    /**
     * Binds the network listeners to the specified interfaces.
     */
    suspend fun bindNetwork(
        quicV4: String? = null,
        quicV6: String? = null,
        tcpV4: String? = null,
        tcpV6: String? = null,
    ) {
        val response = client.invoke(NetworkBind(quicV4, quicV6, tcpV4, tcpV6))
        assert(response == null)
    }

    suspend fun subscribeToNetworkEvents(): EventReceiver<NetworkEvent> =
        client.subscribe(NetworkSubscribe())

    suspend fun quicListenerLocalAddrV4(): String? =
        client.invoke(NetworkQuicListenerLocalAddrV4()) as String?

    suspend fun quicListenerLocalAddrV6(): String? =
        client.invoke(NetworkQuicListenerLocalAddrV6()) as String?

    suspend fun tcpListenerLocalAddrV4(): String? =
        client.invoke(NetworkTcpListenerLocalAddrV4()) as String?

    suspend fun tcpListenerLocalAddrV6(): String? =
        client.invoke(NetworkTcpListenerLocalAddrV6()) as String?

    suspend fun addUserProvidedPeer(addr: String) {
        val response = client.invoke(NetworkAddUserProvidedPeer(addr))
        assert(response == null)
    }

    suspend fun removeUserProvidedPeer(addr: String) {
        val response = client.invoke(NetworkRemoveUserProvidedPeer(addr))
        assert(response == null)
    }

    suspend fun peers(): List<PeerInfo> {
        val list = client.invoke(NetworkKnownPeers()) as List<*>
        return list.map { it as PeerInfo }
    }

    suspend fun currentProtocolVersion(): Int = client.invoke(NetworkCurrentProtocolVersion()) as Int

    suspend fun highestSeenProtocolVersion(): Int = client.invoke(NetworkHighestSeenProtocolVersion()) as Int

    /**
     * Is port forwarding (UPnP) enabled?
     */
    suspend fun isPortForwardingEnabled(): Boolean =
        client.invoke(NetworkIsPortForwardingEnabled()) as Boolean

    /**
     * Enable/disable port forwarding (UPnP)
     */
    suspend fun setPortForwardingEnabled(enabled: Boolean) =
        client.invoke(NetworkSetPortForwardingEnabled(enabled))

    /**
     *  Is local discovery enabled?
     */
    suspend fun isLocalDiscoveryEnabled(): Boolean =
        client.invoke(NetworkIsLocalDiscoveryEnabled()) as Boolean

    /**
     * Enable/disable local discovery
     */
    suspend fun setLocalDiscoveryEnabled(enabled: Boolean) =
        client.invoke(NetworkSetLocalDiscoveryEnabled(enabled))

    /**
     * Returns the runtime id of this ouisync instance.
     *
     * The runtime id is a unique identifier of this instance which is randomly generated on
     * every start.
     */
    suspend fun thisRuntimeId(): String = client.invoke(NetworkThisRuntimeId()) as String

    suspend fun addStorageServer(host: String) =
        client.invoke(NetworkAddStorageServer(host))

    /**
     * Try to gracefully close connections to peers.
     */
    suspend fun shutdownNetwork() = client.invoke(NetworkShutdown())
}
