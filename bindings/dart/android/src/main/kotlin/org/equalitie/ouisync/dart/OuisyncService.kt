package org.equalitie.ouisync.dart

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.util.Log
import kotlin.collections.firstOrNull
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.equalitie.ouisync.kotlin.server.Server

class OuisyncService : Service() {
    // / Local binder allows observing when the service startup completes.
    inner class LocalBinder : Binder() {
        fun onStart(callback: (Result<Unit>) -> Unit) {
            server.invokeOnCompletion { cause ->
                if (cause == null) {
                    callback(Result.success(Unit))
                } else {
                    callback(Result.failure(cause))
                }
            }
        }
    }

    private val scope = CoroutineScope(Dispatchers.Main)
    private var server = CompletableDeferred<Server>()
    private var isStarted = false

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
        start(intent)
        return START_REDELIVER_INTENT
    }

    override fun onBind(intent: Intent?): IBinder? {
        start(intent)
        return LocalBinder()
    }

    private fun start(intent: Intent?) {
        val configPath = intent?.getStringExtra(EXTRA_CONFIG_PATH)
        val debugLabel = intent?.getStringExtra(EXTRA_DEBUG_LABEL)

        if (configPath != null) {
            saveConfigPath(configPath)
        }

        if (isStarted) {
            return
        } else {
            isStarted = true
        }

        scope.launch {
            try {
                server.complete(Server.start(configPath ?: loadConfigPath(), debugLabel))
            } catch (e: Exception) {
                server.completeExceptionally(e)
                stopSelf()
            }
        }

        setupForeground()
    }

    private fun setupForeground() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            createNotificationChannel()
        }

        val notification = createNotification()

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    protected open fun createNotificationChannel() {
        val channel =
            NotificationChannel(
                CHANNEL_ID,
                notificationChannelName,
                NotificationManager.IMPORTANCE_LOW,
            )

        val notificationManager = getSystemService(NotificationManager::class.java)
        notificationManager.createNotificationChannel(channel)
    }

    protected open fun createNotification(): Notification =
        Notification
            .Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.notification_icon)
            .setContentTitle(notificationContentTitle)
            .setOngoing(true)
            .setPriority(Notification.PRIORITY_LOW)
            .setCategory(Notification.CATEGORY_SERVICE)
            .apply {
                val text = notificationContentText
                if (text != null) {
                    setContentText(text)
                }

                val intent = createContentIntent()
                if (intent != null) {
                    setContentIntent(intent)
                }
            }
            .build()

    protected open val notificationChannelName: String = "Ouisync notification channel"
    protected open val notificationContentTitle: String = "Ouisync is running"
    protected open val notificationContentText: String? = null

    private fun createContentIntent(): PendingIntent? {
        val activityClass = getMainActivityClass()
        if (activityClass == null) {
            return null
        }

        val intent = Intent(this, activityClass).apply {
            setAction(Intent.ACTION_MAIN)
            addCategory(Intent.CATEGORY_LAUNCHER)
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }

        val flags = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
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
    protected open fun getMainActivityClass(): Class<*>? = applicationContext.let { context ->
        context.packageManager
            .getPackageInfo(context.packageName, PackageManager.GET_ACTIVITIES)
            .activities
            .asSequence()
            .map { info -> Class.forName(info.name) }
            .filter { klass ->
                val intent = Intent(context, klass).apply {
                    addCategory(Intent.CATEGORY_LAUNCHER)
                }

                context
                    .packageManager
                    .queryIntentActivities(intent, PackageManager.MATCH_DEFAULT_ONLY)
                    .isNotEmpty()
            }.firstOrNull()
    }

    companion object {
        const val EXTRA_CONFIG_PATH = "config-path"
        const val EXTRA_DEBUG_LABEL = "debug-label"
        const val NOTIFICATION_ID = 1
        const val CHANNEL_ID = "org.equalitie.ouisync.plugin.channel"
    }
}
