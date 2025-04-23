package org.equalitie.ouisync.example

import android.content.Context
import android.content.Intent
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Description
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.navigation.NavController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import androidx.navigation.toRoute
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.channels.produce
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import org.equalitie.ouisync.lib.AccessMode
import org.equalitie.ouisync.lib.DirectoryEntry
import org.equalitie.ouisync.lib.EntryType
import org.equalitie.ouisync.lib.File
import org.equalitie.ouisync.lib.Repository
import org.equalitie.ouisync.lib.subscribe
import java.security.MessageDigest

private const val TAG = "ouisync.example"
private val PADDING = 8.dp

@Serializable
object RepositoryListRoute

@Serializable
data class FolderRoute(val repositoryName: String, val path: String = "")

@Serializable
data class FileRoute(val repositoryName: String, val path: String = "")

@Composable
fun ExampleApp(viewModel: ExampleViewModel) {
    val navController = rememberNavController()

    NavHost(
        navController = navController,
        startDestination = RepositoryListRoute,
    ) {
        composable<RepositoryListRoute> {
            RepositoryListScreen(viewModel, navController)
        }

        composable<FolderRoute> { backStackEntry ->
            val route: FolderRoute = backStackEntry.toRoute()

            FolderScreen(
                viewModel = viewModel,
                navController = navController,
                repositoryName = route.repositoryName,
                path = route.path,
            )
        }

        composable<FileRoute> { backStackEntry ->
            val route: FileRoute = backStackEntry.toRoute()

            FileScreen(
                viewModel = viewModel,
                navController = navController,
                repositoryName = route.repositoryName,
                path = route.path,
            )
        }
    }
}

@Composable
fun RepositoryListScreen(
    viewModel: ExampleViewModel,
    navController: NavController,
) {
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    var adding by remember { mutableStateOf(false) }

    Scaffold(
        topBar = { TopBar("Repositories") },
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
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->

        val sessionError = viewModel.sessionError

        if (sessionError == null) {
            RepositoryList(
                repositories = viewModel.repositories,
                onRepositoryClicked = { name ->
                    navController.navigate(route = FolderRoute(name))
                },
                onRepositoryDeleteConfirmed = { name ->
                    scope.launch {
                        viewModel.deleteRepository(name)

                        snackbar.showSnackbar(
                            "Repository '$name' deleted",
                            withDismissAction = true,
                        )
                    }
                },
                modifier = Modifier.padding(padding),
            )
        } else {
            ErrorBox(sessionError, Modifier.padding(padding))
        }

        if (adding) {
            CreateRepositoryDialog(
                repositories = viewModel.repositories,
                onSubmit = { name, token ->

                    adding = false

                    scope.launch {
                        try {
                            viewModel.createRepository(name, token)

                            snackbar.showSnackbar(
                                "Repository created",
                                withDismissAction = true,
                            )
                        } catch (e: Exception) {
                            snackbar.showSnackbar(
                                "Failed to create repository: $e",
                                withDismissAction = true,
                            )
                        }
                    }
                },
                onCancel = {
                    adding = false
                },
            )
        }
    }
}

@Composable
fun RepositoryList(
    repositories: Map<String, Repository>,
    modifier: Modifier = Modifier,
    onRepositoryClicked: (String) -> Unit = {},
    onRepositoryDeleteConfirmed: (String) -> Unit = {},
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var deleting by remember { mutableStateOf<String?>(null) }

    LazyColumn(
        verticalArrangement = Arrangement.spacedBy(PADDING),
        modifier = modifier,
    ) {
        for (entry in repositories) {
            item(key = entry.key) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.padding(PADDING).fillMaxWidth(),
                ) {
                    Text(
                        entry.key,
                        fontWeight = FontWeight.Bold,
                        modifier = Modifier
                            .weight(1f)
                            .clickable { onRepositoryClicked(entry.key) },
                    )

                    IconButton(
                        onClick = {
                            repositories.get(entry.key)?.let { repo ->
                                scope.launch {
                                    shareRepository(context, repo)
                                }
                            }
                        },
                    ) {
                        Icon(Icons.Default.Share, "Share")
                    }

                    IconButton(
                        onClick = {
                            deleting = entry.key
                        },
                    ) {
                        Icon(Icons.Default.Delete, "Delete")
                    }
                }
            }
        }
    }

    deleting?.let { name ->
        DeleteRepositoryDialog(
            onSubmit = {
                onRepositoryDeleteConfirmed(name)
                deleting = null
            },
            onCancel = {
                deleting = null
            },
        )
    }
}

@Composable
fun FolderScreen(
    viewModel: ExampleViewModel,
    navController: NavController,
    repositoryName: String,
    path: String,
) {
    val repo = viewModel.repositories.get(repositoryName)

    Scaffold(
        topBar = { TopBar("$repositoryName$path", navController) },
    ) { padding ->

        if (repo != null) {
            FolderDetail(
                modifier = Modifier.padding(padding),
                repository = repo,
                path = path,
                onEntryClicked = { entry ->
                    when (entry.entryType) {
                        EntryType.FILE -> {
                            navController.navigate(FileRoute(repositoryName, "$path/${entry.name}"))
                        }
                        EntryType.DIRECTORY -> {
                            navController.navigate(FolderRoute(repositoryName, "$path/${entry.name}"))
                        }
                    }
                },
            )
        } else {
            ErrorBox(
                "Repository '$repositoryName' not found",
                Modifier.padding(padding),
            )
        }
    }
}

@Composable
fun FolderDetail(
    modifier: Modifier = Modifier,
    repository: Repository,
    path: String = "",
    onEntryClicked: (DirectoryEntry) -> Unit = {},
) {
    var directory by remember { mutableStateOf<List<DirectoryEntry>>(emptyList()) }

    // Refresh the directory on every notification event from the repo.
    LaunchedEffect(repository, path) {
        directory = repository.readDirectory(path)

        repository.subscribe().collect {
            directory = repository.readDirectory(path)
        }
    }

    LazyColumn(
        verticalArrangement = Arrangement.spacedBy(PADDING),
        modifier = modifier,
    ) {
        for (entry in directory) {
            item(key = entry.name) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.padding(PADDING).fillMaxWidth(),
                ) {
                    when (entry.entryType) {
                        EntryType.FILE -> Icon(Icons.Default.Description, "File")
                        EntryType.DIRECTORY -> Icon(Icons.Default.Folder, "Folder")
                    }

                    Text(
                        entry.name,
                        modifier = Modifier
                            .weight(1f)
                            .clickable { onEntryClicked(entry) },
                        overflow = TextOverflow.Ellipsis,
                        maxLines = 1,
                    )
                }
            }
        }
    }
}

@Composable
fun FileScreen(
    viewModel: ExampleViewModel,
    navController: NavController,
    repositoryName: String,
    path: String,
) {
    val scope = rememberCoroutineScope()
    val repo = viewModel.repositories.get(repositoryName)

    Scaffold(
        topBar = { TopBar("$repositoryName$path", navController) },
    ) { padding ->

        if (repo != null) {
            FileDetail(repo, path, modifier = Modifier.padding(padding))
        } else {
            ErrorBox(
                "Repository '$repositoryName' not found",
                Modifier.padding(padding),
            )
        }
    }
}

@Composable
fun FileDetail(repo: Repository, path: String, modifier: Modifier = Modifier) {
    val scope = rememberCoroutineScope()

    var length by remember { mutableLongStateOf(-1) }
    var error by remember { mutableStateOf<Exception?>(null) }
    var syncProgress by remember { mutableFloatStateOf(0f) }
    var readProgress by remember { mutableFloatStateOf(0f) }
    var hash by remember { mutableStateOf("") }

    // Use `LaunchedEffect` to open, sync and read the file. In a real application this kind of
    // logic would probably be moved to the data layer or a ViewModel. We handle it here for
    // simplicity.
    LaunchedEffect(repo, path) {
        val digest = MessageDigest.getInstance("SHA-256")

        @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
        val events = produce {
            repo.subscribe().collect(::send)
        }

        var maybeFile: File? = null

        while (true) {
            try {
                // Try to open the file. If the file is not synced yet, this throws. We catch the
                // exception, wait for a notification event (which signals that the repository
                // content has changed) and try again.
                val file = repo.openFile(path)

                maybeFile = file
                error = null

                // Obtain the length of the file. If it's different than the last time it means the
                // file has changed and we proceed with the computation. Otherwise we wait for a
                // notification event and try again.
                //
                // Note: It's possible for the file to change while it's length remain the same but
                // we don't handle such situation here for simplicity.
                var newLength = file.getLength()

                if (newLength != length) {
                    length = newLength
                } else {
                    events.receive()
                    continue
                }

                // Reset the UI state
                syncProgress = 0f
                readProgress = 0f
                hash = ""

                // Wait until the file is completelly synced. Reporting the sync progress on each
                // notification event.
                while (true) {
                    val progress = file.getProgress()

                    syncProgress = if (length > 0) {
                        progress.toFloat() / length.toFloat()
                    } else {
                        1f
                    }

                    if (progress >= length) {
                        break
                    }

                    events.receive()
                }

                // Compute the SHA-256 hash of the file iteratively by reading the file in chunks.
                // Report the progress after each processed chunk.
                val chunkLength = 65536L
                var offset = 0L

                while (offset < length) {
                    val chunk = file.read(offset, chunkLength)

                    digest.update(chunk)

                    readProgress = offset.toFloat() / length.toFloat()
                    offset += chunkLength
                }

                readProgress = 1f

                // Update the UI to display the computed hash.
                @OptIn(ExperimentalStdlibApi::class)
                hash = digest.digest().toHexString()
            } catch (e: CancellationException) {
                // The coroutine's been cancelled (likely because it left the composition).

                // Close the file. Because close is a suspend function we need to run it in a
                // non-cancellable context to ensure it runs to completion.
                withContext(NonCancellable) {
                    maybeFile?.close()
                    maybeFile = null
                }

                // Rethrow the exception to complete the cancellation.
                throw e
            } catch (e: Exception) {
                // Show the exception in the UI and reset the rest of the UI state.
                error = e

                length = -1
                syncProgress = 0f
                readProgress = 0f
                hash = ""

                // Wait for a notification event and try again.
                events.receive()
            }
        }

        events.cancel()
    }

    Column(modifier = modifier) {
        if (syncProgress < 1f) {
            FileDetailRow("Syncing") { modifier ->
                if (length < 0) {
                    LinearProgressIndicator(modifier = modifier)
                } else {
                    LinearProgressIndicator(
                        progress = { syncProgress },
                        modifier = modifier,
                    )
                }
            }
        } else if (readProgress < 1f) {
            FileDetailRow("Reading") { modifier ->
                LinearProgressIndicator(
                    progress = { readProgress },
                    modifier = modifier,
                )
            }
        }

        if (length >= 0) {
            FileDetailRow("Size") { modifier ->
                Text(formatSize(length), modifier)
            }
        }

        if (!hash.isEmpty()) {
            FileDetailRow("SHA-256") { modifier ->
                Text(hash, modifier)
            }
        }

        error?.let { error ->
            FileDetailRow("Error") { modifier ->
                Text(error.toString(), modifier)
            }
        }
    }
}

@Composable
fun FileDetailRow(
    label: String,
    content: @Composable (Modifier) -> Unit,
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.padding(PADDING).fillMaxWidth(),
    ) {
        Text(label, Modifier.weight(0.3f).padding(end = PADDING))
        content(Modifier.weight(0.7f))
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TopBar(title: String, navController: NavController? = null) {
    TopAppBar(
        title = {
            Text(
                title,
                // TODO: Use StartEllipsis or MiddleEllipsis when it becomes available
                overflow = TextOverflow.Ellipsis,
                maxLines = 1,
            )
        },
        navigationIcon = {
            if (navController != null) {
                IconButton(onClick = { navController.navigateUp() }) {
                    Icon(Icons.Default.ArrowBack, "Back")
                }
            }
        },
    )
}

@Composable
fun ErrorBox(error: String, modifier: Modifier = Modifier) {
    Box(
        contentAlignment = Alignment.Center,
        modifier = modifier,
    ) {
        Text(error)
    }
}

@Composable
fun CreateRepositoryDialog(
    repositories: Map<String, Repository>,
    onSubmit: (String, String) -> Unit = { _, _ -> },
    onCancel: () -> Unit = {},
) {
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

        if (repositories.containsKey(name)) {
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
                        onSubmit(name, token)
                    }
                },
            ) {
                Text("Create")
            }
        },
        dismissButton = {
            TextButton(onClick = { onCancel() }) {
                Text("Cancel")
            }
        },
        onDismissRequest = { onCancel() },
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

@Composable
fun DeleteRepositoryDialog(onSubmit: () -> Unit = {}, onCancel: () -> Unit = {}) {
    AlertDialog(
        title = {
            Text("Delete repository")
        },
        text = {
            Text("Are you sure you want to delete this repository?")
        },
        onDismissRequest = onCancel,
        confirmButton = {
            TextButton(onClick = onSubmit) {
                Text("Delete")
            }
        },
        dismissButton = {
            TextButton(onClick = onCancel) {
                Text("Cancel")
            }
        },
    )
}

suspend fun shareRepository(context: Context, repo: Repository) {
    val token = repo.share(AccessMode.WRITE).toString()

    val sendIntent = Intent().apply {
        action = Intent.ACTION_SEND
        putExtra(Intent.EXTRA_TEXT, token)
        type = "text/plain"
    }
    val shareIntent = Intent.createChooser(sendIntent, null)

    context.startActivity(shareIntent)
}

fun formatSize(bytes: Long): String {
    val kilo = 1024
    val mega = 1024 * 1024
    val giga = 1024 * 1024 * 1024

    return if (bytes >= giga) {
        "%.1f GiB".format(bytes.toFloat() / giga.toFloat())
    } else if (bytes >= mega) {
        "%.1f MiB".format(bytes.toFloat() / mega.toFloat())
    } else if (bytes >= kilo) {
        "%.1f kiB".format(bytes.toFloat() / kilo.toFloat())
    } else {
        "$bytes B"
    }
}
