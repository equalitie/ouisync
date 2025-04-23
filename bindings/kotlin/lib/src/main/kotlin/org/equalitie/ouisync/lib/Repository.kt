package org.equalitie.ouisync.lib

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.mapNotNull

/**
 * Subscribe to repository events.
 *
 * An even is emitted every time the content of the repo changes (e.g., a file is created,
 * written to, removed, moved, ...).
 */
fun Repository.subscribe(): Flow<Unit> =
    client.subscribe(Request.RepositorySubscribe(handle)).mapNotNull {
        when (it) {
            is Response.RepositoryEvent -> Unit
            is Response.None -> Unit
            else -> null
        }
    }
