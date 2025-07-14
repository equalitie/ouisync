package org.equalitie.ouisync.dart

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.firstOrNull
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.equalitie.ouisync.kotlin.server.Server
import kotlin.collections.firstOrNull

class OuisyncService : Service() {
    private val exceptionHandler =
        CoroutineExceptionHandler { _, e ->
            Log.e(TAG, "uncaught exception in OuisyncService", e)
        }
    private val scope = CoroutineScope(Dispatchers.Main + exceptionHandler)

    private val server: Deferred<Server> = scope.async { Server.start(getConfigPath()) }

    private var isForeground = false

    private val receiver =
        object : BroadcastReceiver() {
            override fun onReceive(
                context: Context,
                intent: Intent,
            ) {
                Log.d(TAG, "OuisyncService.receiver.onReceive(${intent.action})")

                scope.launch {
                    stopServer()
                    stopSelf()
                }
            }
        }

    override fun onCreate() {
        Log.d(TAG, "OuisyncService.onCreate")

        super.onCreate()

        registerReceiver(
            receiver,
            IntentFilter(ACTION_STOP),
            RECEIVER_NOT_EXPORTED,
        )
    }

    override fun onDestroy() {
        Log.d(TAG, "OuisyncService.onDestroy")

        super.onDestroy()

        unregisterReceiver(receiver)
        runBlocking(exceptionHandler) { stopServer() }
    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        Log.d(TAG, "OuisyncService.onStartCommand($intent, $flags, $startId)")

        val notificationChannelName = intent?.getStringExtra(EXTRA_NOTIFICATION_CHANNEL_NAME)
        val notificationContentTitle = intent?.getStringExtra(EXTRA_NOTIFICATION_CONTENT_TITLE)
        val notificationContentText = intent?.getStringExtra(EXTRA_NOTIFICATION_CONTENT_TEXT)

        if (notificationContentTitle != null) {
            setupForeground(
                notificationChannelName,
                notificationContentTitle,
                notificationContentText,
            )
        }

        scope.launch {
            server.await()

            // TODO: consider broadcasting failures as well
            sendBroadcast(Intent(OuisyncService.ACTION_STARTED).setPackage(getPackageName()))
        }

        return START_REDELIVER_INTENT
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private suspend fun stopServer() {
        server.cancel()

        if (server.isCompleted && !server.isCancelled) {
            server.getCompleted().stop()
        }
    }

    private fun setupForeground(
        channelName: String?,
        contentTitle: String,
        contentText: String?,
    ) {
        val manager = getSystemService(NotificationManager::class.java)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            NotificationChannel(
                NOTIFICATION_CHANNEL_ID,
                channelName ?: DEFAULT_NOTIFICATION_CHANNEL_NAME,
                NotificationManager.IMPORTANCE_LOW,
            ).also { channel -> manager.createNotificationChannel(channel) }
        }

        val notification = createNotification(contentTitle, contentText)

        if (isForeground) {
            manager.notify(NOTIFICATION_ID, notification)
        } else {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(
                    NOTIFICATION_ID,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
                )
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }

            isForeground = true
        }
    }

    protected open fun createNotification(
        contentTitle: String? = null,
        contentText: String? = null,
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
            val activities =
                context.packageManager
                    .getPackageInfo(context.packageName, PackageManager.GET_ACTIVITIES)
                    .activities ?: emptyArray()

            activities
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
        const val ACTION_STOP = "org.equalitie.ouisync.service.action.stop"
        const val ACTION_STARTED = "org.equalitie.ouisync.service.action.started"

        const val EXTRA_NOTIFICATION_CHANNEL_NAME =
            "org.equalitie.ouisync.service.extra.notification.channel.name"
        const val EXTRA_NOTIFICATION_CONTENT_TITLE =
            "org.equalitie.ouisync.service.extra.notification.content.title"
        const val EXTRA_NOTIFICATION_CONTENT_TEXT =
            "org.equalitie.ouisync.service.extra.notification.content.text"

        private const val NOTIFICATION_ID = 1
        private const val NOTIFICATION_CHANNEL_ID = "org.equalitie.ouisync.service"
        private const val DEFAULT_NOTIFICATION_CHANNEL_NAME = "Ouisync"
    }
}
