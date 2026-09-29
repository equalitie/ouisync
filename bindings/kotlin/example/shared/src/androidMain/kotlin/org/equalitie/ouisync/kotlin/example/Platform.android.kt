package org.equalitie.ouisync.kotlin.example

import android.content.Intent
import android.util.Log
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext

internal actual fun logDebug(tag: String, message: String) {
    Log.d(tag, message)
}

internal actual fun logError(tag: String, message: String, e: Throwable?) {
    Log.e(tag, message, e)
}

@Composable
internal actual fun rememberShareText(): (String) -> String? {
    val context = LocalContext.current

    return remember(context) {
        { text ->
            // Share using the Android Sharesheet.
            val sendIntent =
                Intent().apply {
                    action = Intent.ACTION_SEND
                    putExtra(Intent.EXTRA_TEXT, text)
                    type = "text/plain"
                }
            val shareIntent = Intent.createChooser(sendIntent, null)

            context.startActivity(shareIntent)

            null
        }
    }
}
