package org.equalitie.ouisync.kotlin.example

import androidx.compose.runtime.Composable

// Platform specific functionality, implemented separately for android (`androidMain`) and desktop
// (`jvmMain`).

internal expect fun logDebug(tag: String, message: String)

internal expect fun logError(tag: String, message: String, e: Throwable? = null)

/**
 * Returns a function which shares the given text with other apps. On android this opens the
 * Sharesheet, on desktop it copies the text to the clipboard. The function returns a message to
 * show to the user, if any.
 */
@Composable internal expect fun rememberShareText(): (String) -> String?
