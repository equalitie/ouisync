package org.equalitie.ouisync.example

import android.os.Bundle
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.equalitie.ouisync.Session

class MainActivity : ComponentActivity() {
    companion object {
        private const val TAG: String = "ouisync.example"
    }

    var session: Session? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        var status: String

        try {
            session = Session.create("${getFilesDir()}/config")
            status = "success"
        } catch (e: Exception) {
            Log.e(TAG, "Session.create failed", e)
            status = "failure ($e)"
        } catch (e: java.lang.Error) {
            Log.e(TAG, "Session.create failed", e)
            status = "failure ($e)"
        }

        setContent {
            Column(Modifier.padding(10.dp)) {
                Text("Ouisync initialization: $status")

                session?.let {
                    ProtocolVersion(it)
                }
            }
        }
    }

    override fun onDestroy() {
        session?.close()
        session = null

        super.onDestroy()
    }
}

@Composable
fun ProtocolVersion(session: Session) {
    val scope = rememberCoroutineScope()
    val (version, setVersion) = remember { mutableStateOf(0) }

    scope.launch {
        setVersion(session.currentProtocolVersion())
    }

    Text("Protocol version: $version")
}
