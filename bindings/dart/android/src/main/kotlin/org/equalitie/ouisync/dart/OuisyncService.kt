package org.equalitie.ouisync.dart

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.equalitie.ouisync.kotlin.server.Server

class OuisyncService : Service() {
    // / Local binder allows observing when the service startup completes.
    inner class LocalBinder : Binder() {
        fun onStart(callback: (Result<Unit>) -> Unit) {
            if (server != null) {
                callback(Result.success(Unit))
            } else {
                startCallbacks.add(callback)
            }
        }
    }

    private val job = SupervisorJob()
    private val scope = CoroutineScope(Dispatchers.Main + job)
    private var server: Server? = null
    private val startCallbacks = mutableListOf<(Result<Unit>) -> Unit>()

    override fun onCreate() {
        super.onCreate()
    }

    override fun onDestroy() {
        super.onDestroy()

        if (server != null) {
            try {
                runBlocking {
                    server?.stop()
                    server = null
                }
            } catch (e: Exception) {
                Log.e(TAG, "failed to stop server", e)
            }
        }

        job.cancel()
    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        val intent = requireNotNull(intent)
        val configPath = requireNotNull(intent.getStringExtra(EXTRA_CONFIG_PATH))
        val debugLabel = intent.getStringExtra(EXTRA_DEBUG_LABEL)

        setupForeground()

        scope.launch {
            try {
                server = Server.start(configPath, debugLabel)
                startCallbacks.forEach { it(Result.success(Unit)) }
            } catch (e: Exception) {
                startCallbacks.forEach { it(Result.failure(e)) }
                stopSelf()
            } finally {
                startCallbacks.clear()
            }
        }

        return START_REDELIVER_INTENT
    }

    override fun onBind(intent: Intent?): IBinder? = LocalBinder()

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
        // TODO: bring the app to the foreground when the notification is tapped
        Notification
            .Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.notification_icon)
            .setContentTitle(notificationContentTitle)
            .apply {
                val text = notificationContentText
                if (text != null) {
                    setContentText(text)
                }
            }.setOngoing(true)
            .setPriority(Notification.PRIORITY_LOW)
            .setCategory(Notification.CATEGORY_SERVICE)
            .build()

    protected open val notificationChannelName: String = "Ouisync notification channel"
    protected open val notificationContentTitle: String = "Ouisync is running"
    protected open val notificationContentText: String? = null

    companion object {
        const val EXTRA_CONFIG_PATH = "config-path"
        const val EXTRA_DEBUG_LABEL = "debug-label"
        const val NOTIFICATION_ID = 1
        const val CHANNEL_ID = "org.equalitie.ouisync.plugin.channel"
    }
}
