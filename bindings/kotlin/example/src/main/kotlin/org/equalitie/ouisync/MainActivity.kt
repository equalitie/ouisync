package org.equalitie.ouisync.example

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val rootDir = getFilesDir()
        val socketPath = "$rootDir/sock"
        val configDir = "$rootDir/config"
        val storeDir = "$rootDir/store"

        val viewModel = ExampleViewModel(socketPath, configDir, storeDir)

        setContent {
            MaterialTheme {
                ExampleApp(viewModel)
            }
        }
    }
}
