package org.equalitie.ouisync.kotlin.example

import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application

fun main(args: Array<String>) {
    // The directory to store the app data in can be passed as the first argument. This is useful
    // for running multiple instances of the app on the same machine (e.g., to test syncing between
    // them).
    val rootDir = args.firstOrNull() ?: "${System.getProperty("user.home")}/.ouisync-kotlin-example"
    val configDir = "$rootDir/config"
    val storeDir = "$rootDir/store"

    val viewModel = ExampleViewModel(configDir, storeDir)

    application {
        Window(
            onCloseRequest = ::exitApplication,
            title = "Ouisync Kotlin bindings example",
        ) {
            MaterialTheme { ExampleApp(viewModel) }
        }
    }
}
