package org.equalitie.ouisync.example

import android.content.Intent
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Description
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.BottomAppBar
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
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
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable
import org.equalitie.ouisync.lib.Directory
import org.equalitie.ouisync.lib.DirectoryEntry
import org.equalitie.ouisync.lib.EntryType
import org.equalitie.ouisync.lib.Repository

private const val TAG = "ouisync.example"
private val PADDING = 8.dp

@Serializable
object RepositoryListRoute

@Serializable
data class RepositoryDetailRoute(val repositoryName: String, val path: String = "")

@Composable
fun ExampleApp(viewModel: ExampleViewModel) {
    val navController = rememberNavController()

    NavHost(
        navController = navController,
        startDestination = RepositoryListRoute,
    ) {
        composable<RepositoryListRoute> {
            RepositoryListScreen(
                viewModel = viewModel,
                navController = navController,
            )
        }

        composable<RepositoryDetailRoute> { backStackEntry ->
            val route: RepositoryDetailRoute = backStackEntry.toRoute()

            RepositoryDetailScreen(
                viewModel = viewModel,
                navController = navController,
                repositoryName = route.repositoryName,
                path = route.path,
            )
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RepositoryListScreen(
    viewModel: ExampleViewModel,
    navController: NavController,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    var adding by remember { mutableStateOf(false) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Repositories") },
            )
        },
        bottomBar = { StatusBar(viewModel) },
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

        RepositoryList(
            repositories = viewModel.repositories,
            onRepositoryClicked = {
                navController.navigate(route = RepositoryDetailRoute(it))
            },
            onRepositoryShareClicked = {
                viewModel.repositories.get(it)?.let {
                    scope.launch {
                        val token = it.createShareToken().toString()

                        val sendIntent = Intent().apply {
                            action = Intent.ACTION_SEND
                            putExtra(Intent.EXTRA_TEXT, token)
                            type = "text/plain"
                        }
                        val shareIntent = Intent.createChooser(sendIntent, null)

                        context.startActivity(shareIntent)
                    }
                }
            },
            onRepositoryDeleteClicked = {
                scope.launch {
                    viewModel.deleteRepository(it)

                    snackbar.showSnackbar(
                        "Repository '$it' deleted",
                        withDismissAction = true,
                    )
                }
            },
            modifier = Modifier.padding(padding),
        )

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
                                "Repository creation failed ($e)",
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
    onRepositoryShareClicked: (String) -> Unit = {},
    onRepositoryDeleteClicked: (String) -> Unit = {},
) {
    LazyColumn(
        verticalArrangement = Arrangement.spacedBy(PADDING),
        modifier = modifier,
    ) {
        for (entry in repositories) {
            item(key = entry.key) {
                RepositoryItem(
                    entry.key,
                    entry.value,
                    onClicked = { onRepositoryClicked(entry.key) },
                    onShareClicked = { onRepositoryShareClicked(entry.key) },
                    onDeleteClicked = { onRepositoryDeleteClicked(entry.key) },
                )
            }
        }
    }
}

@Composable
fun RepositoryItem(
    name: String,
    repository: Repository,
    onClicked: () -> Unit = {},
    onShareClicked: () -> Unit = {},
    onDeleteClicked: () -> Unit = {},
) {
    var deleting by remember { mutableStateOf(false) }

    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.padding(PADDING).fillMaxWidth(),
    ) {
        Text(
            name,
            fontWeight = FontWeight.Bold,
            modifier = Modifier
                .weight(1f)
                .clickable { onClicked() },
        )

        IconButton(
            onClick = {
                onShareClicked()
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

    if (deleting) {
        DeleteRepositoryDialog(
            onSubmit = {
                onDeleteClicked()
                deleting = false
            },
            onCancel = {
                deleting = false
            },
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RepositoryDetailScreen(
    viewModel: ExampleViewModel,
    navController: NavController,
    repositoryName: String,
    path: String,
) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        "$repositoryName$path",
                        // TODO: Use StartEllipsis or MiddleEllipsis when it becomes available
                        overflow = TextOverflow.Ellipsis,
                        maxLines = 1,
                    )
                },
                navigationIcon = {
                    IconButton(onClick = { navController.navigateUp() }) {
                        Icon(Icons.Default.ArrowBack, "Back")
                    }
                },
            )
        },
    ) { padding ->

        RepositoryDetail(
            modifier = Modifier.padding(padding),
            repository = viewModel.repositories.get(repositoryName),
            path = path,
            onEntryClicked = { entry ->
                when (entry.entryType) {
                    EntryType.FILE -> {}
                    EntryType.DIRECTORY -> {
                        navController.navigate(RepositoryDetailRoute(repositoryName, "$path/${entry.name}"))
                    }
                }
            },
        )
    }
}

@Composable
fun RepositoryDetail(
    modifier: Modifier = Modifier,
    repository: Repository? = null,
    path: String = "",
    onEntryClicked: (DirectoryEntry) -> Unit = {},
) {
    var directory by remember { mutableStateOf<Directory>(Directory.empty()) }

    LaunchedEffect(true) {
        repository?.let {
            directory = Directory.open(repository, path)

            it.subscribe().consumeAsFlow().collect {
                directory = Directory.open(repository, path)
            }
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

@Composable
fun StatusBar(viewModel: ExampleViewModel) {
    BottomAppBar {
        val sessionError = viewModel.sessionError

        if (sessionError == null) {
            Icon(Icons.Default.Check, "OK")
            Spacer(modifier = Modifier.weight(1f))
            Text("Protocol version: ${viewModel.protocolVersion}")
        } else {
            Icon(Icons.Default.Warning, "Error")
            Text(sessionError)
        }
    }
}
