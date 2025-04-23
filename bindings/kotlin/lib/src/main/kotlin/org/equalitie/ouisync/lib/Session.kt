package org.equalitie.ouisync.lib

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.mapNotNull

/**
 * Creates a new Ouisync session.
 *
 * @param configPath path to the directory where ouisync stores its config files.
 */
suspend fun Session.Companion.create(configPath: String): Session {
    val client = Client.connect(configPath)
    return Session(client)
}

/**
 * Closes the session.
 *
 * Don't forget to call this when the session is no longer needed, to avoid leaking resources.
 */
suspend fun Session.close() {
    client.close()
}

/**
 * Returns a Flow of network events.
 *
 * Note the event subscription is created only after the flow starts being consumed.
 **/
fun Session.subscribeToNetworkEvents(): Flow<NetworkEvent> =
    client.subscribe(Request.SessionSubscribeToNetwork)
        .mapNotNull {
            when (it) {
                is Response.NetworkEvent -> it.value
                is Response.None -> NetworkEvent.PEER_SET_CHANGE
                else -> null
            }
        }

//     /**
//      * Adds peers to connect to.
//      *
//      * Normally peers are discovered automatically (using Bittorrent DHT, Peer exchange or Local
//      * discovery) but this function is useful in case when the discovery is not available for any
//      * reason (e.g. in an isolated network).
//      *
//      * Note that peers added with this function are remembered across restarts. To forget peers,
//      * use [removeUserProvidedPeers].
//      *
//      * @param addrs addresses of the peers to connect to, in the "PROTOCOL/IP:PORT" format
//      * (PROTOCOL is "tcp" or "quic"). Example: "quic/192.0.2.0:12345"
//      */
//     suspend fun addUserProvidedPeers(addrs: List<String>) =
//         client.invoke(NetworkAddUserProvidedPeers(addrs))

//     /**
//      * Removes peers previously added with [addUserProvidedPeers]
//      */
//     suspend fun removeUserProvidedPeers(addrs: List<String>) =
//         client.invoke(NetworkRemoveUserProvidedPeers(addrs))

//     /**
//      * Returns info about all known peers (both discovered and explicitly added).
//      *
//      * When the set of known peers changes, a [NetworkEvent.PEER_SET_CHANGE] is
//      * emitted. Calling this function afterwards returns the new peer info.
//      *
//      * @see subscribeToNetworkEvents
//      * @see NetworkEvent
//      */
//     suspend fun peers(): List<PeerInfo> {
//         val list = client.invoke(NetworkGetPeers()) as List<*>
//         return list.map { it as PeerInfo }
//     }

//     /**
//      * Returns our Ouisync protocol version.
//      *
//      * In order to establish connections with peers, they must use the same protocol version as us.
//      *
//      * @see highestSeenProtocolVersion
//      */
//     suspend fun currentProtocolVersion(): Long = client.invoke(NetworkGetCurrentProtocolVersion()) as Long

//     /**
//      * Returns the highest protocol version of all known peers. If this is higher than
//      * [our version](currentProtocolVersion) it likely means we are using an outdated version of
//      * Ouisync. When a peer with higher protocol version is found, a
//      * [NetworkEvent.PROTOCOL_VERSION_MISMATCH] is emitted.
//      */
//     suspend fun highestSeenProtocolVersion(): Long = client.invoke(NetworkGetHighestSeenProtocolVersion()) as Long

//     /**
//      * Is port forwarding (UPnP) enabled?
//      */
//     suspend fun isPortForwardingEnabled(): Boolean =
//         client.invoke(NetworkIsPortForwardingEnabled()) as Boolean

//     /**
//      * Enable/disable port forwarding (UPnP)
//      */
//     suspend fun setPortForwardingEnabled(enabled: Boolean) =
//         client.invoke(NetworkSetPortForwardingEnabled(enabled))

//     /**
//      *  Is local discovery enabled?
//      */
//     suspend fun isLocalDiscoveryEnabled(): Boolean =
//         client.invoke(NetworkIsLocalDiscoveryEnabled()) as Boolean

//     /**
//      * Enable/disable local discovery
//      */
//     suspend fun setLocalDiscoveryEnabled(enabled: Boolean) =
//         client.invoke(NetworkSetLocalDiscoveryEnabled(enabled))

//     /**
//      * Returns the runtime id of this Ouisync instance.
//      *
//      * The runtime id is a unique identifier of this instance which is randomly generated on
//      * every start.
//      */
//     suspend fun thisRuntimeId(): String = client.invoke(NetworkGetRuntimeId()) as String
// }
