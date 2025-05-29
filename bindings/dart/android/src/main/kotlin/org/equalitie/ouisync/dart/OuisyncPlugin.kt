package org.equalitie.ouisync.dart

import android.app.Activity
import android.app.Service
import android.content.ActivityNotFoundException
import android.content.ComponentName
import android.content.Intent
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import io.flutter.embedding.engine.plugins.FlutterPlugin
import io.flutter.embedding.engine.plugins.activity.ActivityAware
import io.flutter.embedding.engine.plugins.activity.ActivityPluginBinding
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.MethodChannel.MethodCallHandler
import org.equalitie.ouisync.kotlin.client.LogLevel
import org.equalitie.ouisync.kotlin.server.initLog

internal const val TAG = "ouisync"

class OuisyncPlugin :
    FlutterPlugin,
    MethodCallHandler,
    ActivityAware,
    ServiceConnection {
    var channel: MethodChannel? = null

    private val mainHandler = Handler(Looper.getMainLooper())
    private var activity: Activity? = null
    private var startCallbacks = mutableListOf<(Result<Unit>) -> Unit>()

    companion object {
        private const val CHANNEL_NAME = "org.equalitie.ouisync.plugin"
    }

    override fun onAttachedToActivity(binding: ActivityPluginBinding) {
        val activity = binding.activity

        requestPermissions(activity)

        activity.bindService(
            Intent(activity, OuisyncService::class.java),
            this,
            Service.BIND_AUTO_CREATE
        )

        this.activity = activity
    }

    override fun onDetachedFromActivity() {
        activity?.unbindService(this)
        activity = null
    }

    override fun onDetachedFromActivityForConfigChanges() {
        onDetachedFromActivity()
    }

    override fun onReattachedToActivityForConfigChanges(binding: ActivityPluginBinding) {
        onAttachedToActivity(binding)
    }

    private fun requestPermissions(activity: Activity) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            return
        }

        if (ContextCompat.checkSelfPermission(
                activity,
                android.Manifest.permission.POST_NOTIFICATIONS,
            ) == PackageManager.PERMISSION_GRANTED
        ) {
            return
        }

        ActivityCompat.requestPermissions(
            activity,
            arrayOf(android.Manifest.permission.POST_NOTIFICATIONS),
            1,
        )
    }

    override fun onAttachedToEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        channel =
            MethodChannel(binding.binaryMessenger, CHANNEL_NAME).also { it.setMethodCallHandler(this) }
    }

    override fun onDetachedFromEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        channel?.let { it.setMethodCallHandler(null) }
        channel = null
    }

    override fun onServiceConnected(
        name: ComponentName,
        binder: IBinder,
    ) {
        (binder as OuisyncService.LocalBinder).onStart {
            val result = it.map { Unit }
            startCallbacks.forEach { callback -> callback(result) }
            startCallbacks.clear()
        }
    }

    override fun onServiceDisconnected(name: ComponentName) = Unit

    override fun onMethodCall(
        call: MethodCall,
        result: MethodChannel.Result,
    ) {
        when (call.method) {
            "initLog" -> {
                val arguments = call.arguments as Map<String, Any?>
                val stdout = arguments["stdout"] as Boolean
                val file = arguments["file"] as String?
                onInitLog(stdout, file)
                result.success(null)
            }
            "start" -> {
                val arguments = call.arguments as Map<String, Any?>
                val configPath = arguments["configPath"] as String
                val debugLabel = arguments["debugLabel"] as String?
                val notificationChannelName = arguments["notificationChannelName"] as String?
                val notificationContentTitle = arguments["notificationContentTitle"] as String?
                val notificationContentText = arguments["notificationContentText"] as String?

                onStart(
                    configPath,
                    debugLabel,
                    notificationChannelName,
                    notificationContentTitle,
                    notificationContentText,
                ) {
                    // TODO: do we need to explicitly call `result.error` on failure?
                    it.getOrThrow()
                    result.success(null)
                }
            }
            "stop" -> {
                onStop()
                result.success(null)
            }
            "viewFile" -> {
                val uri = Uri.parse(call.arguments as String)
                result.success(onViewFile(uri))
            }
            "shareFile" -> {
                val uri = Uri.parse(call.arguments as String)
                result.success(onShareFile(uri))
            }
            else -> {
                result.notImplemented()
            }
        }
    }

    private fun onInitLog(
        stdout: Boolean,
        file: String?,
    ) {
        if (stdout) {
            initLog(
                stdout = false,
                file = file,
                callback = { level, message -> Log.println(logPriority(level), TAG, message) },
            )
        } else {
            initLog(
                file = file,
                callback = { level, message ->
                    mainHandler.post {
                        channel?.invokeMethod("log", mapOf("level" to level.toValue(), "message" to message))
                    }
                },
            )
        }
    }

    private fun onStart(
        configPath: String,
        debugLabel: String?,
        notificationChannelName: String?,
        notificationContentTitle: String?,
        notificationContentText: String?,
        callback: (Result<Unit>) -> Unit,
    ) {
        val activity = requireNotNull(this.activity)

        startCallbacks.add(callback)

        activity.startForegroundService(
            Intent(activity, OuisyncService::class.java).apply {
                putExtra(OuisyncService.EXTRA_CONFIG_PATH, configPath)
                putExtra(OuisyncService.EXTRA_DEBUG_LABEL, debugLabel)
                putExtra(OuisyncService.EXTRA_NOTIFICATION_CHANNEL_NAME, notificationChannelName)
                putExtra(OuisyncService.EXTRA_NOTIFICATION_CONTENT_TITLE, notificationContentTitle)
                putExtra(OuisyncService.EXTRA_NOTIFICATION_CONTENT_TEXT, notificationContentText)
            },
        )
    }

    private fun onStop() {
        activity?.let { activity ->
            activity.unbindService(this)
            activity.stopService(Intent(activity, OuisyncService::class.java))
        }
    }

    private fun onViewFile(uri: Uri): Boolean {
        val context = requireNotNull(activity)
        val mimeType = context.contentResolver.getType(uri) ?: "application/octet-stream"
        val intent =
            Intent(Intent.ACTION_VIEW)
                // Some apps (e.g., Google Files) can't open the file unless we specify the type
                // explicitly.
                .setDataAndType(uri, mimeType)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)

        try {
            context.startActivity(intent)
        } catch (e: ActivityNotFoundException) {
            Log.d(TAG, "no default app found for $uri")
            return false
        } catch (e: Exception) {
            Log.e(TAG, "failed to start activity", e)
            throw e
        }

        return true
    }

    private fun onShareFile(uri: Uri) {
        val context = requireNotNull(activity)
        val intent =
            Intent(Intent.ACTION_SEND)
                .setType("*/*")
                .putExtra(Intent.EXTRA_STREAM, uri)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)

        context.startActivity(Intent.createChooser(intent, null))
    }
}

private fun logPriority(level: LogLevel) =
    when (level) {
        LogLevel.ERROR -> Log.ERROR
        LogLevel.WARN -> Log.WARN
        LogLevel.INFO -> Log.INFO
        LogLevel.DEBUG -> Log.DEBUG
        LogLevel.TRACE -> Log.VERBOSE
    }
