package org.equalitie.ouisync

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
                callback,
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
        val response = client.invoke(
            NetworkInit(
                defaultLocalDiscoveryEnabled,
                defaultPortForwardingEnabled,
            ),
        )

        assert(response == null)
    }

    suspend fun bindNetwork(
        quicV4: String? = null,
        quicV6: String? = null,
        tcpV4: String? = null,
        tcpV6: String? = null,
    ) {
        val response = client.invoke(NetworkBind(quicV4, quicV6, tcpV4, tcpV6))
        assert(response == null)
    }

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

    // Future<void> addStorageServer(String host) =>
    //     client.invoke<void>('network_add_storage_server', host);

    // /// Destroys the session.
    // Future<void> dispose() async {
    //   if (debugTrace) {
    //     print("Session.dispose");
    //   }

    //   await _networkSubscription.close();

    //   if (handle != 0) {
    //     b.bindings.session_destroy(handle);
    //     NativeChannels.session = null;
    //   }
    // }

    // /// Try to gracefully close connections to peers.
    // Future<void> shutdownNetwork() async {
    //   await client.invoke<void>('network_shutdown');
    // }
}
