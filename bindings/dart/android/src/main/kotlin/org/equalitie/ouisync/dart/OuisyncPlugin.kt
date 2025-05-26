package org.equalitie.ouisync.dart

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ComponentName
import android.content.Context
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
import java.net.URLConnection

internal const val TAG = "ouisync"

/** OuisyncPlugin */
class OuisyncPlugin :
    FlutterPlugin,
    MethodCallHandler,
    ActivityAware,
    ServiceConnection {
    private var context: Context? = null
    private val connectionCallbacks = mutableListOf<(OuisyncService.LocalBinder) -> Unit>()

    // To run stuff on the main thread.
    private val mainHandler = Handler(Looper.getMainLooper())

    var activity: Activity? = null
    var channel: MethodChannel? = null

    companion object {
        private const val CHANNEL_NAME = "org.equalitie.ouisync.plugin"

        private var channels = HashSet<MethodChannel>()

        // Each instance of this class has its own method channel, but one of them is also accessible
        // via this getter. Only those instances that are currently attached to an activity can have
        // their channel shared here. This is because an instance can also be attached to a Service
        // which might not have created the other (dart) end of the channel and so such channel would
        // not be usable.
        val sharedChannel: MethodChannel?
            get() = synchronized(channels) { channels.firstOrNull() }

        private fun enableSharingForChannel(channel: MethodChannel) = synchronized(channels) { channels.add(channel) }

        private fun disableSharingForChannel(channel: MethodChannel) = synchronized(channels) { channels.remove(channel) }
    }

    override fun onAttachedToActivity(binding: ActivityPluginBinding) {
        requestPermissions(binding.activity)

        activity = binding.activity

        channel?.let { enableSharingForChannel(it) }
    }

    override fun onDetachedFromActivity() {
        activity = null

        channel?.let { disableSharingForChannel(it) }
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
        context = binding.getApplicationContext()

        channel =
            MethodChannel(binding.binaryMessenger, CHANNEL_NAME).also {
                it.setMethodCallHandler(this)

                if (activity != null) {
                    enableSharingForChannel(it)
                }
            }
    }

    override fun onDetachedFromEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        channel?.let {
            disableSharingForChannel(it)
            it.setMethodCallHandler(null)
        }
        channel = null

        context?.unbindService(this)
        context = null
    }

    override fun onServiceConnected(
        name: ComponentName,
        binder: IBinder,
    ) {
        val serviceBinder = binder as OuisyncService.LocalBinder

        connectionCallbacks.forEach { it(serviceBinder) }
        connectionCallbacks.clear()
    }

    override fun onServiceDisconnected(name: ComponentName) {}

    override fun onMethodCall(
        call: MethodCall,
        result: MethodChannel.Result,
    ) {
        val arguments = call.arguments as Map<String, Any?>

        when (call.method) {
            "initLog" -> {
                val stdout = arguments["stdout"] as Boolean
                val file = arguments["file"] as String?
                onInitLog(stdout, file)
                result.success(null)
            }
            "start" -> {
                val configPath = arguments["configPath"] as String
                val debugLabel = arguments["debugLabel"] as String?
                onStart(configPath, debugLabel) {
                    // TODO: do we need to explicitly call `result.error` on failure?
                    it.getOrThrow()
                    result.success(null)
                }
            }
            "stop" -> {
                onStop()
                result.success(null)
            }
            "shareFile" -> {
                startFileShareAction(arguments)
                result.success("Share file intent started")
            }
            "previewFile" -> {
                val previewResult = startFilePreviewAction(arguments)
                result.success(previewResult)
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
        callback: (Result<Unit>) -> Unit,
    ) {
        val context = requireNotNull(this.context)

        connectionCallbacks.add { binder -> binder.onStart(callback) }

        // start the service so that it can outlive this plugin instance...
        context.startForegroundService(
            Intent(context, OuisyncService::class.java).apply {
                putExtra(OuisyncService.EXTRA_CONFIG_PATH, configPath)
                putExtra(OuisyncService.EXTRA_DEBUG_LABEL, debugLabel)
            },
        )

        // ...but also bind to it so we can observe when the service startup completes.
        context.bindService(Intent(context, OuisyncService::class.java), this, 0)
    }

    private fun onStop() {
        val context = requireNotNull(this.context)

        context.unbindService(this)
        context.stopService(Intent(context, OuisyncService::class.java))
    }

    private fun startFilePreviewAction(arguments: Map<String, Any?>): String {
        val authority = arguments["authority"]
        val path = arguments["path"] as String
        val size = arguments["size"]
        val useDefaultApp = arguments["useDefaultApp"] as Boolean? ?: false

        val uri = Uri.parse("content://$authority.pipe/$size$path")
        val mimeType = URLConnection.guessContentTypeFromName(path) ?: "application/octet-stream"

        val intent =
            Intent(Intent.ACTION_VIEW)
                // Some apps (e.g., Google Files) can't open the file unless we specify the type
                // explicitly.
                .setDataAndType(uri, mimeType)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)

        try {
            if (useDefaultApp) {
                // Note that not using Intent.createChooser let's the user choose a
                // default app and then use that the next time the same file type is
                // opened.
                activity?.startActivity(intent)
            } else {
                val title = "Preview file from Ouisync"
                activity?.startActivity(Intent.createChooser(intent, title))
            }
        } catch (e: ActivityNotFoundException) {
            Log.d(javaClass.simpleName, "Exception: No default app for this file type was found")
            return "noDefaultApp"
        }

        return "previewOK"
    }

    private fun startFileShareAction(arguments: Map<String, Any?>) {
        val authority = arguments["authority"]
        val path = arguments["path"]
        val size = arguments["size"]
        val title = "Share file from Ouisync"

        val uri = Uri.parse("content://$authority.pipe/$size$path")

        Log.d(javaClass.simpleName, "Uri: $uri")

        val intent =
            Intent(Intent.ACTION_SEND)
                .setType("*/*")
                .putExtra(Intent.EXTRA_STREAM, uri)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)

        activity?.startActivity(Intent.createChooser(intent, title))
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
