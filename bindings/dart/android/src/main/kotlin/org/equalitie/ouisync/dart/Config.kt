package org.equalitie.ouisync.dart

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume

private const val PREFS_NAME = "org.equalitie.ouisync.plugin"
private const val CONFIG_PATH_KEY = "config-path"

// Save the given config path to SharedPreferences.
internal fun Context.saveConfigPath(path: String) = getSharedPreferences(PREFS_NAME, 0).edit().putString(CONFIG_PATH_KEY, path).commit()

// Loads the config path from SharedPreferences. If it's not yet stored, waits until it is.
internal suspend fun Context.loadConfigPath(): String {
    val prefs = getSharedPreferences(PREFS_NAME, 0)
    var listener: SharedPreferences.OnSharedPreferenceChangeListener? = null

    return suspendCancellableCoroutine<String> { cont ->
        listener =
            object : SharedPreferences.OnSharedPreferenceChangeListener {
                override fun onSharedPreferenceChanged(
                    prefs: SharedPreferences,
                    key: String?,
                ) {
                    if (key != CONFIG_PATH_KEY) {
                        return
                    }

                    prefs.getString(key, null)?.let {
                        cont.resume(it)
                        prefs.unregisterOnSharedPreferenceChangeListener(listener)
                    }
                }
            }

        prefs.registerOnSharedPreferenceChangeListener(listener)

        cont.invokeOnCancellation { prefs.unregisterOnSharedPreferenceChangeListener(listener) }

        prefs.getString(CONFIG_PATH_KEY, null)?.let { cont.resume(it) }
    }
}
