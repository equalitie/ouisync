package org.equalitie.ouisync.session

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.mapNotNull

/**
 * Creates a new Ouisync session and connects it to a
 * [Service][org.equalitie.ouisync.service.Service].
 *
 * @param configPath path to the config directory of the service to connect to. The session only
 *   needs read access to it.
 */
suspend fun Session.Companion.create(configPath: String): Session {
    val client = Client.connect(configPath)
    return Session(client)
}

/**
 * Closes the session.
 *
 * This should be called when the session is no longer needed to prevent memory leaks.
 */
suspend fun Session.close() {
    client.close()
}

/**
 * Returns a Flow of [network events][NetworkEvent].
 *
 * Note the event subscription is created only after the flow starts being consumed.
 */
fun Session.subscribeToNetworkEvents(): Flow<NetworkEvent> = client.subscribe(Request.SessionSubscribeToNetwork).mapNotNull {
    when (it) {
        is Response.NetworkEvent -> it.value
        is Response.None -> NetworkEvent.PEER_SET_CHANGE
        else -> null
    }
}
