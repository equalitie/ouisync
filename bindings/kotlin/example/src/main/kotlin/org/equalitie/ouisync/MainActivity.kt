package org.equalitie.ouisync.example

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.viewModels
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.BottomAppBar
import androidx.compose.material3.Card
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import org.equalitie.ouisync.Repository
import org.equalitie.ouisync.Session
import org.equalitie.ouisync.ShareToken
import java.io.File

private const val TAG = "ouisync.example"
private val PADDING = 8.dp

private val DB_EXTENSION = "ouisyncdb"

class MainActivity : ComponentActivity() {
    private val viewModel by viewModels<AppViewModel>() {
        AppViewModel.Factory(this)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        setContent {
            App(viewModel)
        }
    }
}

class AppViewModel(private val configDir: String, private val storeDir: String) : ViewModel() {
    class Factory(private val context: Context) : ViewModelProvider.Factory {
        @Suppress("UNCHECKED_CAST")
        override fun <T : ViewModel> create(modelClass: Class<T>): T {
            val rootDir = context.getFilesDir()
            val configDir = "$rootDir/config"
            val storeDir = "$rootDir/store"

            return AppViewModel(configDir, storeDir) as T
        }
    }

    var sessionError by mutableStateOf<String?>(null)
        private set

    var protocolVersion by mutableStateOf<Int>(0)
        private set

    var repositories by mutableStateOf<Map<String, Repository>>(mapOf())
        private set

    private var session: Session? = null

    init {
        try {
            session = Session.create(configDir)
            sessionError = null
        } catch (e: Exception) {
            Log.e(TAG, "Session.create failed", e)
            sessionError = e.toString()
        } catch (e: java.lang.Error) {
            Log.e(TAG, "Session.create failed", e)
            sessionError = e.toString()
        }

        viewModelScope.launch {
            session?.let {
                protocolVersion = it.currentProtocolVersion()
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
            shareToken = ShareToken.fromString(session, token)
        }

        val repo = Repository.create(
            session,
            "$storeDir/$name.$DB_EXTENSION",
            readSecret = null,
            writeSecret = null,
            shareToken = shareToken,
        )

        repositories = repositories + (name to repo)
    }

    suspend fun deleteRepository(name: String) {
        val repo = repositories.get(name) ?: return
        repositories = repositories - name

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

                    Log.i(TAG, "Opened repository $name")

                    repositories = repositories + (name to repo)
                } catch (e: Exception) {
                    Log.e(TAG, "Failed to open repository at ${file.getPath()}")
                    continue
                }
            }
        }
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

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun App(viewModel: AppViewModel) {
    val scope = rememberCoroutineScope()
    val snackbar = remember { Snackbar(scope) }
    var adding by remember { mutableStateOf(false) }

    MaterialTheme {
        Scaffold(
            floatingActionButton = {
                if (!adding) {
                    FloatingActionButton(
                        onClick = {
                            adding = true
                        },
                    ) {
                        Icon(Icons.Default.Add, "Add")
                    }
                }
            },
            bottomBar = { StatusBar(viewModel) },
            snackbarHost = { SnackbarHost(snackbar.state) },
            content = { padding ->
                Column(
                    modifier = Modifier
                        .fillMaxSize()
                        .padding(PADDING),
                ) {
                    viewModel.sessionError?.let {
                        Text(it)
                    }

                    RepositoryList(viewModel, snackbar = snackbar)

                    if (adding) {
                        CreateRepositoryDialog(
                            viewModel,
                            snackbar = snackbar,
                            onDone = { adding = false },
                        )
                    }
                }
            },
        )
    }
}

@Composable
fun StatusBar(viewModel: AppViewModel) {
    BottomAppBar {
        if (viewModel.sessionError == null) {
            Icon(Icons.Default.Check, "OK")
            Spacer(modifier = Modifier.weight(1f))
            Text("Protocol version: ${viewModel.protocolVersion}")
        } else {
            Icon(Icons.Default.Warning, "Error")
        }
    }
}

@Composable
fun RepositoryList(viewModel: AppViewModel, snackbar: Snackbar) {
    val scope = rememberCoroutineScope()

    LazyColumn(
        verticalArrangement = Arrangement.spacedBy(PADDING),
    ) {
        for (entry in viewModel.repositories) {
            item(key = entry.key) {
                RepositoryItem(
                    entry.key,
                    entry.value,
                    onDelete = {
                        scope.launch {
                            viewModel.deleteRepository(entry.key)
                            snackbar.show("Repository deleted")
                        }
                    },
                )
            }
        }
    }
}

@Composable
fun RepositoryItem(
    name: String,
    repository: Repository,
    onDelete: () -> Unit,
) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    var deleting by remember { mutableStateOf(false) }

    suspend fun sendShareToken() {
        val token = repository.createShareToken().toString()

        val sendIntent = Intent().apply {
            action = Intent.ACTION_SEND
            putExtra(Intent.EXTRA_TEXT, token)
            type = "text/plain"
        }
        val shareIntent = Intent.createChooser(sendIntent, null)

        context.startActivity(shareIntent)
    }

    Card(modifier = Modifier.fillMaxWidth()) {
        Row(modifier = Modifier.padding(PADDING)) {
            Text(name, fontWeight = FontWeight.Bold)

            Spacer(Modifier.weight(1f))

            IconButton(
                onClick = {
                    scope.launch {
                        sendShareToken()
                    }
                },
            ) {
                Icon(Icons.Default.Share, "Share")
            }

            IconButton(
                onClick = {
                    deleting = true
                },
            ) {
                Icon(Icons.Default.Delete, "Delete")
            }
        }
    }

    if (deleting) {
        AlertDialog(
            title = {
                Text("Delete repository")
            },
            text = {
                Text("Are you sure you want to delete this repository?")
            },
            onDismissRequest = { deleting = false },
            confirmButton = {
                TextButton(
                    onClick = {
                        onDelete()
                        deleting = false
                    },
                ) {
                    Text("Delete")
                }
            },
            dismissButton = {
                TextButton(
                    onClick = { deleting = false },
                ) {
                    Text("Cancel")
                }
            },
        )
    }
}

@Composable
fun CreateRepositoryDialog(
    viewModel: AppViewModel,
    onDone: () -> Unit,
    snackbar: Snackbar,
) {
    var scope = rememberCoroutineScope()

    var name by remember {
        mutableStateOf("")
    }

    var nameError by remember {
        mutableStateOf("")
    }

    var token by remember {
        mutableStateOf("")
    }

    fun validate(): Boolean {
        if (name.isEmpty()) {
            nameError = "Name is missing"
            return false
        }

        if (viewModel.repositories.containsKey(name)) {
            nameError = "Name is already taken"
            return false
        }

        nameError = ""
        return true
    }

    AlertDialog(
        title = { Text("Create repository") },
        confirmButton = {
            TextButton(
                onClick = {
                    if (validate()) {
                        scope.launch {
                            try {
                                viewModel.createRepository(name, token)
                                snackbar.show("Repository created")
                            } catch (e: Exception) {
                                snackbar.show("Repository creation failed ($e)")
                            } finally {
                                onDone()
                            }
                        }
                    }
                },
            ) {
                Text("Create")
            }
        },
        dismissButton = {
            TextButton(onClick = { onDone() }) {
                Text("Cancel")
            }
        },
        onDismissRequest = { onDone() },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(PADDING)) {
                TextField(
                    value = name,
                    onValueChange = { name = it },
                    label = { Text("Name*") },
                    supportingText = {
                        if (!nameError.isEmpty()) {
                            Text(nameError)
                        }
                    },
                    isError = !nameError.isEmpty(),
                )

                TextField(
                    label = { Text("Token") },
                    value = token,
                    onValueChange = { token = it },
                )
            }
        },
    )
}

class Snackbar(val scope: CoroutineScope) {
    val state = SnackbarHostState()

    fun show(text: String) {
        scope.launch {
            state.showSnackbar(text, withDismissAction = true)
        }
    }
}
