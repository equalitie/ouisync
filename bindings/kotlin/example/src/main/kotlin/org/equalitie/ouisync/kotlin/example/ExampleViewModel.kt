package org.equalitie.ouisync.kotlin.example

import android.util.Log
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
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

    init {
        initLog()

        viewModelScope.launch(Dispatchers.IO) {
            try {
                service = Service.start(configDir)
            } catch (e: OuisyncException.ServiceAlreadyRunning) {
                Log.d(TAG, "Service already running")
            } catch (e: Exception) {
                Log.e(TAG, "Service.start failed", e)
                sessionError = e.toString()
            }

            if (service != null) {
                try {
                    session = Session.create(configDir)
                    session?.setStoreDir(storeDir)
                } catch (e: Exception) {
                    Log.e(TAG, "Session.create failed", e)
                    sessionError = e.toString()
                } catch (e: java.lang.Error) {
                    Log.e(TAG, "Session.create failed", e)
                    sessionError = e.toString()
                }
            }

            session?.let {
                // Bind the network sockets to all interfaces and random ports. Use only the QUIC
                // protocol and use both IPv4 and IPv6.
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
            Log.e(TAG, "repository named \"$name\" already exists")
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

    override fun onCleared() {
        val repos = repositories.values
        repositories = mapOf()

        viewModelScope.launch {
            for (repo in repos) {
                repo.close()
            }

            session?.close()
            session = null
        }
    }
}
