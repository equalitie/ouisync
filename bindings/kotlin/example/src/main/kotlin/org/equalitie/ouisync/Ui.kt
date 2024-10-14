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
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.navigation.NavController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import org.equalitie.ouisync.lib.Repository

private const val TAG = "ouisync.example"
private val PADDING = 8.dp

enum class ExampleScreen(val title: String) {
    RepositoryList("Repositories"),
    RepositoryDetail("Repository"),
}

@Composable
fun ExampleApp(viewModel: ExampleViewModel) {
    val navController = rememberNavController()

    NavHost(
        navController = navController,
        startDestination = ExampleScreen.RepositoryList.name,
    ) {
        composable(route = ExampleScreen.RepositoryList.name) {
            RepositoryListScreen(
                viewModel = viewModel,
                navController = navController,
            )
        }

        composable(route = ExampleScreen.RepositoryDetail.name) {
            RepositoryDetail(
                viewModel = viewModel,
                navController = navController,
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
    val scope = rememberCoroutineScope()
    val snackbar = remember { Snackbar(scope) }
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
        snackbarHost = { SnackbarHost(snackbar.state) },
    ) { padding ->

        RepositoryList(
            viewModel = viewModel,
            onRepositoryClicked = {
                // TODO
            },
            onRepositoryDeleted = {
                snackbar.show("Repository '$it' deleted")
            },
            modifier = Modifier.padding(padding),
        )

        if (adding) {
            CreateRepositoryDialog(
                viewModel,
                onSuccess = {
                    adding = false
                    snackbar.show("Repository created")
                },
                onFailure = { error ->
                    adding = false
                    snackbar.show("Repository creation failed ($error)")
                },
                onDismiss = {
                    adding = false
                },
            )
        }
    }
}

@Composable
fun RepositoryList(
    viewModel: ExampleViewModel,
    onRepositoryClicked: (String) -> Unit = {},
    onRepositoryDeleted: (String) -> Unit = {},
    modifier: Modifier = Modifier,
) {
    val scope = rememberCoroutineScope()

    LazyColumn(
        verticalArrangement = Arrangement.spacedBy(PADDING),
        modifier = modifier,
    ) {
        for (entry in viewModel.repositories) {
            item(key = entry.key) {
                RepositoryItem(
                    entry.key,
                    entry.value,
                    onClicked = {
                        // ...
                    },
                    onDeleteClicked = {
                        scope.launch {
                            viewModel.deleteRepository(entry.key)
                        }

                        onRepositoryDeleted(entry.key)
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
    onClicked: () -> Unit = {},
    onDeleteClicked: () -> Unit = {},
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

    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.padding(PADDING).fillMaxWidth(),
    ) {
        Text(
            name,
            fontWeight = FontWeight.Bold,
            modifier = Modifier
                .weight(1f)
                .clickable { onClicked() }
        )

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
                        onDeleteClicked()
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
fun RepositoryDetail(viewModel: ExampleViewModel, navController: NavController) {

}

@Composable
fun CreateRepositoryDialog(
    viewModel: ExampleViewModel,
    onSuccess: () -> Unit = {},
    onFailure: (Exception) -> Unit = {},
    onDismiss: () -> Unit = {},
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
                                onSuccess()
                            } catch (e: Exception) {
                                onFailure(e)
                            }
                        }
                    }
                },
            ) {
                Text("Create")
            }
        },
        dismissButton = {
            TextButton(onClick = { onDismiss() }) {
                Text("Cancel")
            }
        },
        onDismissRequest = { onDismiss() },
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

class Snackbar(val scope: CoroutineScope) {
    val state = SnackbarHostState()

    fun show(text: String) {
        scope.launch {
            state.showSnackbar(text, withDismissAction = true)
        }
    }
}
