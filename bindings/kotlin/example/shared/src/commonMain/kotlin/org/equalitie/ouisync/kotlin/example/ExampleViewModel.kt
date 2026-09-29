package org.equalitie.ouisync.kotlin.example

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.DelicateCoroutinesApi
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.GlobalScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import org.equalitie.ouisync.service.Service
import org.equalitie.ouisync.service.initLog
import org.equalitie.ouisync.session.OuisyncException
import org.equalitie.ouisync.session.Repository
import org.equalitie.ouisync.session.Session
import org.equalitie.ouisync.session.ShareToken
import org.equalitie.ouisync.session.close
import org.equalitie.ouisync.session.create

private const val TAG = "ouisync.example"

class ExampleViewModel(
    private val configDir: String,
    private val storeDir: String,
) : ViewModel() {
    private var service: Service? = null
    private var session: Session? = null

    var sessionError by mutableStateOf<String?>(null)
        private set

    var repositories by mutableStateOf<Map<String, Repository>>(mapOf())
        private set

    private val initJob: Job

    init {
        initLog()

        initJob =
            viewModelScope.launch(Dispatchers.IO) {
                try {
                    service = Service.start(configDir)
                } catch (e: OuisyncException.ServiceAlreadyRunning) {
                    logDebug(TAG, "Service already running")
                } catch (e: CancellationException) {
                    // Cancelled by `close`. Not an error, just propagate it.
                    throw e
                } catch (e: Exception) {
                    logError(TAG, "Service.start failed", e)
                    sessionError = e.toString()
                }

                // Create the session unless the service failed to start. Note that if the service
                // is already running (e.g., started by a previous instance of this view model which
                // hasn't been closed yet), the session connects to it.
                if (sessionError == null) {
                    try {
                        session = Session.create(configDir)
                        session?.setStoreDirs(listOf(storeDir))
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        logError(TAG, "Session.create failed", e)
                        sessionError = e.toString()
                    } catch (e: java.lang.Error) {
                        logError(TAG, "Session.create failed", e)
                        sessionError = e.toString()
                    }
                }

                session?.let {
                    // Bind the network sockets to all interfaces and random ports. Use only the
                    // QUIC protocol and use both IPv4 and IPv6.
                    it.bindNetwork(listOf("quic/0.0.0.0:0", "quic/[::]:0"))

                    // Enable port forwarding (UPnP) to improve chances of connecting to peers.
                    it.setPortForwardingEnabled(true)

                    // Enable Local Disocvery to automatically discover peers on the local network.
                    it.setLocalDiscoveryEnabled(true)
                }

                openRepositories()
            }
    }

    suspend fun createRepository(name: String, token: String) {
        val session = this.session ?: return

        if (repositories.containsKey(name)) {
            logError(TAG, "repository named \"$name\" already exists")
            return
        }

        var shareToken: ShareToken? = null

        if (!token.isEmpty()) {
            shareToken = session.validateShareToken(token)
        }

        val repo = session.createRepository(name, token = shareToken)

        // Syncing is initially disabled, need to enable it.
        repo.setSyncEnabled(true)

        // Enable DHT and PEX for discovering peers. These settings are persisted so it's not
        // necessary to set them again when opening the repository later.
        repo.setDhtEnabled(true)
        repo.setPexEnabled(true)

        repositories = repositories + (name to repo)
    }

    suspend fun deleteRepository(name: String) {
        val repo = repositories.get(name) ?: return

        repositories -= name
        repo.delete()
    }

    private suspend fun openRepositories() {
        val session = this.session ?: return
        repositories = repositories + session.listRepositories()
    }

    /**
     * Closes the session and stops the service if this view model started it (which also closes all
     * the repositories). Call this before the app exits.
     */
    suspend fun close() {
        // Wait for the initialization to finish (or cancel it if it's still running) to avoid
        // racing with it.
        initJob.cancelAndJoin()

        repositories = mapOf()

        session?.close()
        session = null

        service?.stop()
        service = null
    }

    override fun onCleared() {
        // `viewModelScope` is already cancelled at this point, so launch the cleanup in a scope
        // which outlives this view model.
        @OptIn(DelicateCoroutinesApi::class)
        GlobalScope.launch { close() }
    }
}
