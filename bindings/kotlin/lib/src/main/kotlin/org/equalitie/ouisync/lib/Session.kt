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
