package org.equalitie.ouisync.kotlin.example

import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application
import kotlinx.coroutines.runBlocking
import kotlin.system.exitProcess

fun main(args: Array<String>) {
    // The directory to store the app data in can be passed as the first argument. This is useful
    // for running multiple instances of the app on the same machine (e.g., to test syncing between
    // them).
    val rootDir = args.firstOrNull() ?: "${System.getProperty("user.home")}/.ouisync-kotlin-example"
    val configDir = "$rootDir/config"
    val storeDir = "$rootDir/store"

    val viewModel = ExampleViewModel(configDir, storeDir)

    // Don't exit the process immediately when the app exits so we can clean up first.
    application(exitProcessOnExit = false) {
        Window(
            onCloseRequest = ::exitApplication,
            title = "Ouisync Kotlin bindings example",
        ) {
            MaterialTheme { ExampleApp(viewModel) }
        }
    }

    // The view model is not managed by any `ViewModelStore` here (so `onCleared` is never called),
    // so close it explicitly.
    runBlocking { viewModel.close() }

    exitProcess(0)
}
