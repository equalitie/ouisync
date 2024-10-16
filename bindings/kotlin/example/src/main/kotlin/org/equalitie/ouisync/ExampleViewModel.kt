package org.equalitie.ouisync.example

import android.util.Log
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.getAndUpdate
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import org.equalitie.ouisync.lib.Repository
import org.equalitie.ouisync.lib.Session
import org.equalitie.ouisync.lib.ShareToken
import java.io.File

private val DB_EXTENSION = "ouisyncdb"
private const val TAG = "ouisync.example"

data class UiState(
    val sessionError: String? = null,
    val repositories: Map<String, Repository> = mapOf(),
)

class ExampleViewModel(private val configDir: String, private val storeDir: String) : ViewModel() {
    private val _state = MutableStateFlow(UiState())
    val state: StateFlow<UiState> = _state.asStateFlow()

    private var session: Session? = null

    init {
        try {
            session = Session.create(configDir)
        } catch (e: Exception) {
            Log.e(TAG, "Session.create failed", e)

            _state.update {
                it.copy(sessionError = e.toString())
            }
        } catch (e: java.lang.Error) {
            Log.e(TAG, "Session.create failed", e)

            _state.update {
                it.copy(sessionError = e.toString())
            }
        }

        viewModelScope.launch {
            session?.let {
                // Bind the network sockets to all interfaces and random ports. Use only the QUIC
                // protocol and use both IPv4 and IPv6.
                it.bindNetwork(quicV4 = "0.0.0.0:0", quicV6 = "[::]:0")

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

        if (_state.value.repositories.containsKey(name)) {
            Log.e(TAG, "repository named \"$name\" already exists")
            return
        }

        var shareToken: ShareToken? = null

        if (!token.isEmpty()) {
            shareToken = ShareToken.fromString(session, token)
        }

        val repo = Repository.create(
            session,
            "$storeDir/$name.$DB_EXTENSION",
            readSecret = null,
            writeSecret = null,
            shareToken = shareToken,
        )

        repo.setSyncEnabled(true)

        _state.update {
            it.copy(repositories = it.repositories + (name to repo))
        }
    }

    suspend fun deleteRepository(name: String) {
        val repo = _state.value.repositories.get(name) ?: return

        _state.update {
            it.copy(repositories = it.repositories - name)
        }

        repo.close()

        val baseName = "$name.$DB_EXTENSION"
        val files = File(storeDir).listFiles() ?: arrayOf()

        // A ouisync repository database consist of multiple files. Delete all of them.
        for (file in files) {
            if (file.getName().startsWith(baseName)) {
                file.delete()
            }
        }
    }

    private suspend fun openRepositories() {
        val session = this.session ?: return
        val files = File(storeDir).listFiles() ?: arrayOf()

        for (file in files) {
            if (file.getName().endsWith(".$DB_EXTENSION")) {
                try {
                    val name = file
                        .getName()
                        .substring(0, file.getName().length - DB_EXTENSION.length - 1)
                    val repo = Repository.open(session, file.getPath())

                    repo.setSyncEnabled(true)

                    Log.i(TAG, "Opened repository $name")

                    _state.update {
                        it.copy(repositories = it.repositories + (name to repo))
                    }
                } catch (e: Exception) {
                    Log.e(TAG, "Failed to open repository at ${file.getPath()}")
                    continue
                }
            }
        }
    }

    override fun onCleared() {
        val state = _state.getAndUpdate {
            it.copy(sessionError = null, repositories = mapOf())
        }

        viewModelScope.launch {
            for (repo in state.repositories.values) {
                repo.close()
            }

            session?.close()
            session = null
        }
    }
}
