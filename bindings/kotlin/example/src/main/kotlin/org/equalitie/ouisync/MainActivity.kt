package org.equalitie.ouisync.example

import android.os.Bundle
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.material3.Text
import org.equalitie.ouisync.Session

class MainActivity : ComponentActivity() {
    companion object {
        private const val TAG: String = "ouisync.example"
    }

    var session: Session? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        var status = ""

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
            Text("Ouisync initialization: $status")
        }
    }

    override fun onDestroy() {
        session?.close()
        session = null

        super.onDestroy()
    }
}
