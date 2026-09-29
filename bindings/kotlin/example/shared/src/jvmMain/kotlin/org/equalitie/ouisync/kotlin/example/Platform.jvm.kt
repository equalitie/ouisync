package org.equalitie.ouisync.kotlin.example

import androidx.compose.runtime.Composable
import java.awt.Toolkit
import java.awt.datatransfer.StringSelection

internal actual fun logDebug(tag: String, message: String) {
    println("D/$tag: $message")
}

internal actual fun logError(tag: String, message: String, e: Throwable?) {
    System.err.println("E/$tag: $message")
    e?.printStackTrace()
}

@Composable
internal actual fun rememberShareText(): (String) -> String? = { text ->
    // There is no system-wide share dialog on desktop, so just copy the text to the clipboard.
    Toolkit.getDefaultToolkit().systemClipboard.setContents(StringSelection(text), null)
    "Copied to clipboard"
}
