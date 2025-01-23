package org.equalitie.ouisync.lib

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.filterIsInstance
import kotlinx.coroutines.flow.mapNotNull

/**
 * The entry point to the ouisync library.
 *
 * ## Example usage
 *
 * ```
 * // Create a session and initialize networking:
 * val session = Session.create("path/to/config/dir")
 * session.initNetwork(listOf("quic/0.0.0.0:0", "quic/[::]:0"))
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
         * @param configPath path to the directory where ouisync stores its config files.
         * @param debugLabel label used to distinguish multiple [Session] instances for debug
         * purposes. Useful mostly for tests and can be ignored otherwise.
         *
         * @throws Error
         */
        suspend fun create(
            configPath: String,
            debugLabel: String? = null,
        ): Session {
            var server: Server? = null

            try {
                server = Server.start(configPath, debugLabel)
            } catch (e: Error.ServiceAlreadyRunning) {
                server = null
            }

            val client = Client.connect(configPath)

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

    suspend fun setStoreDir(path: String) = client.invoke(RepositorySetStoreDir(path))

    suspend fun storeDir(): String? = client.invoke(RepositoryGetStoreDir()) as String?

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
    ) = client.invoke(
        NetworkInit(
            defaultBindAddrs,
            defaultLocalDiscoveryEnabled,
            defaultPortForwardingEnabled,
        ),
    )

    /**
     * Binds the network listeners to the specified interfaces.
     *
     * Up to four listeners can be bound, one for each combination of protocol (TCP or QUIC) and IP
     * family (IPv4 or IPv6). The format of the interfaces is "PROTO/IP:PORT" where PROTO is "tcp"
     * or "quic". If IP is IPv6, it needs to be enclosed in square brackets.
     *
     * If port is `0`, binds to a random port initially but on subsequent starts tries to use the
     * same port (unless it's already taken). This can be useful to configuring port forwarding.
     *
     * ## Example
     *
     * ```
     * session.bindNetwork(listOf("quic/0.0.0.0:0", "quic/[::]:0"))
     * ```
     */
    suspend fun bindNetwork(addrs: List<String>) = client.invoke(NetworkBind(addrs))

    /**
     * Returns a Flow of network events.
     *
     * Note the event subscription is created only after the flow starts being consumed.
     **/
    fun subscribeToNetworkEvents(): Flow<NetworkEvent> =
        client.subscribe(NetworkSubscribe())
            .mapNotNull {
                when (it) {
                    is NetworkEvent -> it
                    is Any -> NetworkEvent.PEER_SET_CHANGE
                    else -> null
                }
            }

    /**
     * Returns the listener interface addresses.
     */
    suspend fun networkListenerAddrs(): List<String> {
        val list = client.invoke(NetworkGetListenerAddrs()) as List<*>
        return list.filterIsInstance<String>()
    }

    /**
     * Adds peers to connect to.
     *
     * Normally peers are discovered automatically (using Bittorrent DHT, Peer exchange or Local
     * discovery) but this function is useful in case when the discovery is not available for any
     * reason (e.g. in an isolated network).
     *
     * Note that peers added with this function are remembered across restarts. To forget peers,
     * use [removeUserProvidedPeers].
     *
     * @param addrs addresses of the peers to connect to, in the "PROTOCOL/IP:PORT" format
     * (PROTOCOL is "tcp" or "quic"). Example: "quic/192.0.2.0:12345"
     */
    suspend fun addUserProvidedPeers(addrs: List<String>) =
        client.invoke(NetworkAddUserProvidedPeers(addrs))

    /**
     * Removes peers previously added with [addUserProvidedPeers]
     */
    suspend fun removeUserProvidedPeers(addrs: List<String>) =
        client.invoke(NetworkRemoveUserProvidedPeers(addrs))

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
        val list = client.invoke(NetworkGetPeers()) as List<*>
        return list.map { it as PeerInfo }
    }

    /**
     * Returns our Ouisync protocol version.
     *
     * In order to establish connections with peers, they must use the same protocol version as us.
     *
     * @see highestSeenProtocolVersion
     */
    suspend fun currentProtocolVersion(): Long = client.invoke(NetworkGetCurrentProtocolVersion()) as Long

    /**
     * Returns the highest protocol version of all known peers. If this is higher than
     * [our version](currentProtocolVersion) it likely means we are using an outdated version of
     * Ouisync. When a peer with higher protocol version is found, a
     * [NetworkEvent.PROTOCOL_VERSION_MISMATCH] is emitted.
     */
    suspend fun highestSeenProtocolVersion(): Long = client.invoke(NetworkGetHighestSeenProtocolVersion()) as Long

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
    suspend fun thisRuntimeId(): String = client.invoke(NetworkGetRuntimeId()) as String
}
