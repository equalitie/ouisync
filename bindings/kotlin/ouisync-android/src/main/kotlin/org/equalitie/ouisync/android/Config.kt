package org.equalitie.ouisync.android

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map

private const val CONFIG_NAME = "org.equalitie.ouisync"
private val CONFIG_PATH_KEY = stringPreferencesKey("config_path")

private val Context.config: DataStore<Preferences> by preferencesDataStore(CONFIG_NAME)

suspend fun Context.getConfigPath() = applicationContext.config.data.map { prefs -> prefs[CONFIG_PATH_KEY] }.filterNotNull().first()

suspend fun Context.setConfigPath(value: String?) = applicationContext.config.edit { prefs ->
    if (value != null) {
        prefs[CONFIG_PATH_KEY] = value
    } else {
        prefs.remove(CONFIG_PATH_KEY)
    }
}
