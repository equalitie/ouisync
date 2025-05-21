package org.equalitie.ouisync.dart

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.util.Log
import io.flutter.embedding.engine.plugins.FlutterPlugin
import io.flutter.embedding.engine.plugins.activity.ActivityAware
import io.flutter.embedding.engine.plugins.activity.ActivityPluginBinding
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.MethodChannel.MethodCallHandler
import io.flutter.plugin.common.MethodChannel.Result
import kotlinx.coroutines.runBlocking
import org.equalitie.ouisync.kotlin.client.LogLevel
import org.equalitie.ouisync.kotlin.server.Server
import org.equalitie.ouisync.kotlin.server.initLog
import java.net.URLConnection

/** OuisyncPlugin */
class OuisyncPlugin :
    FlutterPlugin,
    MethodCallHandler,
    ActivityAware {
    // / The MethodChannel that will the communication between Flutter and native Android
    // /
    // / This local reference serves to register the plugin with the Flutter Engine and unregister it
    // / when the Flutter Engine is detached from the Activity
    var activity: Activity? = null
    var channel: MethodChannel? = null

    var servers: MutableMap<Int, Server> = mutableMapOf()

    companion object {
        private val TAG = OuisyncPlugin::class.java.simpleName
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

    override fun onAttachedToEngine(binding: FlutterPlugin.FlutterPluginBinding) {
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

        val servers = synchronized(servers) {
            servers.values.also {
                servers.clear()
            }
        }

        runBlocking {
            for (server in servers) {
                try {
                    server.stop()
                } catch (e: Exception) {
                    Log.e(TAG, "failed to stop server", e)
                }
            }
        }
    }

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
                val handle = onStart(configPath, debugLabel)
                result.success(handle)
            }
            "stop" -> {
                val handle = arguments["handle"] as Int
                onStop(handle)
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
                    channel?.invokeMethod("log", mapOf("level" to level.toValue(), "message" to message))
                },
            )
        }
    }

    private fun onStart(
        configPath: String,
        debugLabel: String?,
    ): Int {
        val server = runBlocking { Server.start(configPath, debugLabel) }

        synchronized(servers) {
            val handle = servers.size + 1
            servers.put(handle, server)
            return handle
        }
    }

    private fun onStop(handle: Int) {
        val server = synchronized(servers) { servers.remove(handle) }

        if (server == null) {
            return
        }

        runBlocking { server.stop() }
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
