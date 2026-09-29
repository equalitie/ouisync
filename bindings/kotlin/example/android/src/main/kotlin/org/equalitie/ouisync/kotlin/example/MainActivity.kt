package org.equalitie.ouisync.kotlin.example

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import androidx.lifecycle.viewmodel.compose.viewModel

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val rootDir = getFilesDir()
        val configDir = "$rootDir/config"
        val storeDir = "$rootDir/store"

        setContent {
            // Create the view model using `viewModel` so that it survives configuration changes
            // (e.g., screen rotation) and gets cleared (see `ExampleViewModel.onCleared`) when the
            // activity is finished.
            val viewModel = viewModel { ExampleViewModel(configDir, storeDir) }

            MaterialTheme { ExampleApp(viewModel) }
        }
    }
}
