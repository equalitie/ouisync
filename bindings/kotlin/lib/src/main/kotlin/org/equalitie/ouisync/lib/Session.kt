package org.equalitie.ouisync.lib

import kotlinx.coroutines.flow.map

/**
 * The entry point to the ouisync library.
 *
 * ## Example usage
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
    internal val client: Client,
    private val server: Server?,
) {
    companion object {

        /**
         * Creates a new Ouisync session.
         *
         * @param socketPath TODO
         * @param configPath path to the directory where ouisync stores its config files.
         * @param debugLabel label used to distinguish multiple [Session] instances for debug
         * purposes. Useful mostly for tests and can be ignored otherwise.
         *
         * @throws Error
         */
        suspend fun create(
            socketPath: String,
            configPath: String,
            debugLabel: String? = null,
        ): Session {
            var server: Server? = null

            try {
                server = Server.start(socketPath, configPath, debugLabel)
            } catch (e: ServiceAlreadyRunning) {
                server = null
            }

            val client = Client.connect(socketPath)

            return Session(client, server)
        }
    }

    /**
     * Closes the session.
     *
     * Don't forget to call this when the session is no longer needed, to avoid leaking resources.
     */
    suspend fun close() {
        client.close()
        server?.stop()
    }

    /**
     * Initializes the network according to the stored config. If no config exists, falls back to
     * the given parameters.
     *
     * @param defaultBindAddrs             addresses to bind the listeners to by default.
     * @param defaultPortForwardingEnabled whether Port Forwarding/UPnP should be enabled by default.
     * @param defaultLocalDiscoveryEnabled whether Local Discovery should be enabled by default.
     *
     * @see isPortForwardingEnabled
     * @see setPortForwardingEnabled
     * @see isLocalDiscoveryEnabled
     * @see setLocalDiscoveryEnabled
     */
    suspend fun initNetwork(
        defaultBindAddrs: List<String> = listOf(),
        defaultPortForwardingEnabled: Boolean,
        defaultLocalDiscoveryEnabled: Boolean,
    ) {
        val response = client.invoke(
            NetworkInit(
                defaultBindAddrs,
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
     * enclosed in square brackets.
     *
     * If port is `0`, binds to a random port initially but on subsequent starts tries to use the
     * same port (unless it's already taken). This can be useful to configuring port forwarding.
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
     * Note that peers added with this function are remembered across restarts. To forget a peer,
     * use [removeUserProvidedPeer].
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
     *
     * When the set of known peers changes, a [NetworkEvent.PEER_SET_CHANGE] is
     * emitted. Calling this function afterwards returns the new peer info.
     *
     * @see subscribeToNetworkEvents
     * @see NetworkEvent
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
     * Ouisync. When a peer with higher protocol version is found, a
     * [NetworkEvent.PROTOCOL_VERSION_MISMATCH] is emitted.
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
}
