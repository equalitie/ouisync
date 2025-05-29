package org.equalitie.ouisync.dart

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Binder
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import android.util.Log
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.firstOrNull
import kotlinx.coroutines.flow.map
import org.equalitie.ouisync.kotlin.server.Server
import kotlin.collections.firstOrNull


class OuisyncService : Service() {
    // Local binder allows observing when the service startup completes.
    inner class LocalBinder : Binder() {
        // Invokes the callback when the server has been started.
        fun onStart(callback: (Result<String>) -> Unit) {
            scope.launch {
                try {
                    server.await()
                    val configPath = config.get(EXTRA_CONFIG_PATH.stringKey)
                    callback(Result.success(configPath))
                } catch (e: Throwable) {
                    callback(Result.failure(e))
                }
            }
        }
    }

    private val scope = CoroutineScope(Dispatchers.Main)

    private val config: DataStore<Preferences> by preferencesDataStore(CONFIG_NAME)

    private val server: Deferred<Server> = scope.async {
        val configPath = config.get(EXTRA_CONFIG_PATH.stringKey)
        val debugLabel = config.getOrNull(EXTRA_DEBUG_LABEL.stringKey)

        Server.start(configPath, debugLabel)
    }

    override fun onCreate() {
        super.onCreate()
    }

    override fun onDestroy() {
        super.onDestroy()

        runBlocking {
            scope.cancel()

            if (server.isCompleted) {
                try {
                    server.getCompleted().stop()
                } catch (e: Exception) {
                    Log.e(TAG, "failed to stop server", e)
                }
            }
        }
    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        scope.launch {
            updateConfig(intent?.extras ?: Bundle.EMPTY)
            setupForeground()
            server.await()
        }

        return START_REDELIVER_INTENT
    }

    override fun onBind(intent: Intent?): IBinder? {
        scope.launch {
            setupForeground()
            server.await()
        }

        return LocalBinder()
    }

    private suspend fun updateConfig(extras: Bundle) = config.edit { prefs ->
        for (name in arrayOf(
            EXTRA_CONFIG_PATH,
            EXTRA_DEBUG_LABEL,
            EXTRA_NOTIFICATION_CHANNEL_NAME,
            EXTRA_NOTIFICATION_CONTENT_TITLE,
            EXTRA_NOTIFICATION_CONTENT_TEXT
        )) {
            val value = extras.getString(name)

            if (value != null) {
                prefs[name.stringKey] = value
            }
        }
    }

    private suspend fun setupForeground() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channelName = config.getOrNull(EXTRA_NOTIFICATION_CHANNEL_NAME.stringKey) ?: DEFAULT_NOTIFICATION_CHANNEL_NAME
            createNotificationChannel(channelName)
        }

        val notification = createNotification(
            contentTitle = config.getOrNull(EXTRA_NOTIFICATION_CONTENT_TITLE.stringKey),
            contentText  = config.getOrNull(EXTRA_NOTIFICATION_CONTENT_TEXT.stringKey),
        )

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    protected open fun createNotificationChannel(name: String) {
        val channel = NotificationChannel(
            NOTIFICATION_CHANNEL_ID,
            name,
            NotificationManager.IMPORTANCE_LOW
        )

        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    protected open fun createNotification(
        contentTitle: String? = null,
        contentText: String? = null
    ): Notification =
        Notification
            .Builder(this, NOTIFICATION_CHANNEL_ID)
            .setSmallIcon(R.mipmap.ouisync_notification_icon)
            .setOngoing(true)
            .setPriority(Notification.PRIORITY_LOW)
            .setCategory(Notification.CATEGORY_SERVICE)
            .apply {
                if (contentTitle != null) {
                    setContentTitle(contentTitle)
                }

                if (contentText != null) {
                    setContentText(contentText)
                }

                val intent = createContentIntent()
                if (intent != null) {
                    setContentIntent(intent)
                }
            }.build()

    private fun createContentIntent(): PendingIntent? {
        val activityClass = getMainActivityClass()
        if (activityClass == null) {
            return null
        }

        val intent =
            Intent(this, activityClass).apply {
                setAction(Intent.ACTION_MAIN)
                addCategory(Intent.CATEGORY_LAUNCHER)
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }

        val flags =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                PendingIntent.FLAG_IMMUTABLE
            } else {
                0
            }

        return PendingIntent.getActivity(this, 0, intent, flags)
    }

    // Returns the class of the "main" activity of the current application. This assumes there is
    // only one launchable activity in the app. If that's not the case, it arbitrarily returns one
    // such activity which might not be what you want. In such cases it's recommented to override
    // this method.
    protected open fun getMainActivityClass(): Class<*>? =
        applicationContext.let { context ->
            context.packageManager
                .getPackageInfo(context.packageName, PackageManager.GET_ACTIVITIES)
                .activities
                .asSequence()
                .map { info -> Class.forName(info.name) }
                .filter { klass ->
                    val intent = Intent(context, klass).apply { addCategory(Intent.CATEGORY_LAUNCHER) }

                    context.packageManager
                        .queryIntentActivities(intent, PackageManager.MATCH_DEFAULT_ONLY)
                        .isNotEmpty()
                }.firstOrNull()
        }

    companion object {
        const val EXTRA_CONFIG_PATH = "org.equalitie.ouisync.service.extra.config_path"
        const val EXTRA_DEBUG_LABEL = "org.equalitie.ouisync.service.extra.debug_label"
        const val EXTRA_NOTIFICATION_CHANNEL_NAME = "org.equalitie.ouisync.service.extra.notification_channel_name"
        const val EXTRA_NOTIFICATION_CONTENT_TITLE = "org.equalitie.ouisync.service.extra.notification_content_title"
        const val EXTRA_NOTIFICATION_CONTENT_TEXT = "org.equalitie.ouisync.service.extra.notification_content_text"

        private const val CONFIG_NAME = "org.equalitie.ouisync.service"

        private const val NOTIFICATION_ID = 1
        private const val NOTIFICATION_CHANNEL_ID = "org.equalitie.ouisync.service"
        private const val DEFAULT_NOTIFICATION_CHANNEL_NAME = "Ouisync"
    }
}

private val String.stringKey: Preferences.Key<String>
    get() = stringPreferencesKey(substringAfterLast('.'))

private suspend fun <T> DataStore<Preferences>.get(key: Preferences.Key<T>): T =
    data.map { prefs -> prefs[key] }.filterNotNull().first()

private suspend fun <T> DataStore<Preferences>.getOrNull(key: Preferences.Key<T>): T? =
    data.map { prefs -> prefs[key] }.firstOrNull()

