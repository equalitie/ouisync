package org.equalitie.ouisync

import com.sun.jna.Pointer
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.runBlocking
import java.io.Closeable

/**
 * The entry point to the ouisync library.
 *
 * Example usage:
 *
 * ```
 * // Create a session and initialize networking:
 * val session = Session.create("path/to/config/dir")
 * session.initNetwork()
 * session.bind(quicV4 = "0.0.0.0:0", quicV6 = "[::]:0")
 *
 * // ...
 *
 * // When done, close it:
 * session.close()
 * ```
 */
class Session private constructor(
    private val handle: Long,
    internal val client: Client,
    private val callback: Callback,
) : Closeable {
    companion object {
        internal val bindings = Bindings.INSTANCE

        /**
         * Creates a new Ouisync session.
         *
         * @param configsPath path to the directory where ouisync stores its config files.
         * @param logPath path to the log file. Ouisync always logs using the
         *                [android log API](https://developer.android.com/reference/android/util/Log)
         *                but if this param is not null, it logs to the specified file as well.
         * @throws Error
         */
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
                // Keep a reference to the callback in this session to ensure it doesn't get
                // garbage collected prematurely. More info:
                // https://github.com/java-native-access/jna/blob/master/www/CallbacksAndClosures.md
                return Session(result.handle, client, callback)
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

    /**
     * Closes the session.
     *
     * Don't forget to call this when the session is no longer needed, to avoid leaking resources.
     *
     * @see shutdownNetwork to gracefully disconnect from the peer before closing the session.
     */
    override fun close() {
        bindings.session_close(handle)
    }

    /**
     * Initializes the network according to the stored config. If not config exists, falls back to
     * the given parameters.
     *
     * @param defaultPortForwardingEnabled whether Port Forwarding/UPnP should be enabled by default.
     * @param defaultLocalDiscoveryEnabled whether Local Discovery should be enabled by default.
     *
     * @see isPortForwardingEnabled
     * @see setPortForwardingEnabled
     * @see isLocalDiscoveryEnabled
     * @see setLocalDiscoveryEnabled
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
     *
     * Up to four listeners can be bound, one for each combination of protocol (TCP or QUIC) and IP
     * family (IPv4 or IPv6). Specify the interfaces as "IP:PORT". If IP is IPv6, it needs to be
     * enclosed in square brackets. It's recommended to use "unspecified" interface and a random
     * port: IPv4: "0.0.0.0:0", IPv6: "[[::]]:0".
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

    /**
     * Subscribe to the network events.
     **/
    suspend fun subscribeToNetworkEvents(): EventReceiver<NetworkEvent> =
        client.subscribe(NetworkSubscribe())

    /**
     * Returns the interface the QUIC IPv4 listener is bound to, if any.
     */
    suspend fun quicListenerLocalAddrV4(): String? =
        client.invoke(NetworkQuicListenerLocalAddrV4()) as String?

    /**
     * Returns the interface the QUIC IPv6 listener is bound to, if any.
     */
    suspend fun quicListenerLocalAddrV6(): String? =
        client.invoke(NetworkQuicListenerLocalAddrV6()) as String?

    /**
     * Returns the interface the TCP IPv4 listener is bound to, if any.
     */
    suspend fun tcpListenerLocalAddrV4(): String? =
        client.invoke(NetworkTcpListenerLocalAddrV4()) as String?

    /**
     * Returns the interface the TCP IPv6 listener is bound to, if any.
     */
    suspend fun tcpListenerLocalAddrV6(): String? =
        client.invoke(NetworkTcpListenerLocalAddrV6()) as String?

    /**
     * Adds a peer to connect to.
     *
     * Normally peers are discovered automatically (using Bittorrent DHT, Peer exchange or Local
     * discovery) but this function is useful in case when the discovery is not available for any
     * reason (e.g. in an isolated network).
     *
     * @param addr address of the peer to connect to, in the "PROTOCOL/IP:PORT" format (PROTOCOL is
     * is "tcp" or "quic"). Example: "quic/192.0.2.0:12345"
     */
    suspend fun addUserProvidedPeer(addr: String) {
        val response = client.invoke(NetworkAddUserProvidedPeer(addr))
        assert(response == null)
    }

    /**
     * Removes a peer previously added with [addUserProvidedPeer]
     */
    suspend fun removeUserProvidedPeer(addr: String) {
        val response = client.invoke(NetworkRemoveUserProvidedPeer(addr))
        assert(response == null)
    }

    /**
     * Returns info about all known peers (both discovered and explicitly added).
     */
    suspend fun peers(): List<PeerInfo> {
        val list = client.invoke(NetworkKnownPeers()) as List<*>
        return list.map { it as PeerInfo }
    }

    /**
     * Returns our Ouisync protocol version.
     *
     * In order to establish connections with peers, they must use the same protocol version as us.
     *
     * @see highestSeenProtocolVersion
     */
    suspend fun currentProtocolVersion(): Int = client.invoke(NetworkCurrentProtocolVersion()) as Int

    /**
     * Returns the highest protocol version of all known peers. If this is higher than
     * [our version](currentProtocolVersion) it likely means we are using an outdated version of
     * Ouisync.
     */
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
     * Returns the runtime id of this Ouisync instance.
     *
     * The runtime id is a unique identifier of this instance which is randomly generated on
     * every start.
     */
    suspend fun thisRuntimeId(): String = client.invoke(NetworkThisRuntimeId()) as String

    /**
     * Adds a "cache server" to use.
     *
     * Cache servers relay traffic between Ouisync peers and also temporarily store data. They are
     * useful when direct P2P connection fails (e.g. due to restrictive NAT) and also to allow
     * syncing when the peers are not online at the same time (they still need to be online within
     * ~24 hours of each other).
     *
     * @see Repository.mirror to create a "mirror" of a repository on the cache server.
     */
    suspend fun addCacheServer(host: String) =
        client.invoke(NetworkAddStorageServer(host))

    /**
     * Try to gracefully close all the connections to the peers.
     *
     * It's advisable to call this before [closing the session][close] so the peers get immediately
     * notified about the connections closing without having to wait for them to timeout.
     */
    suspend fun shutdownNetwork() = client.invoke(NetworkShutdown())
}
