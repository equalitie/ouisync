package ie.equalit.ouisync_plugin

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.os.Environment
import io.flutter.embedding.engine.plugins.FlutterPlugin
import io.flutter.embedding.engine.plugins.activity.ActivityAware
import io.flutter.embedding.engine.plugins.activity.ActivityPluginBinding
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.MethodChannel.MethodCallHandler
import io.flutter.plugin.common.MethodChannel.Result
import java.net.URLConnection

/** OuisyncPlugin */
class OuisyncPlugin: FlutterPlugin, MethodCallHandler, ActivityAware {
  /// The MethodChannel that will the communication between Flutter and native Android
  ///
  /// This local reference serves to register the plugin with the Flutter Engine and unregister it
  /// when the Flutter Engine is detached from the Activity
  var activity : Activity? = null
  var channel : MethodChannel? = null

  companion object {
    private val TAG = OuisyncPlugin::class.java.simpleName
    private const val CHANNEL_NAME = "ie.equalit.ouisync_plugin"

    private var channels = HashSet<MethodChannel>()

    // Each instance of this class has its own method channel, but one of them is also accessible
    // via this getter. Only those instances that are currently attached to an activity can have
    // their channel shared here. This is because an instance can also be attached to a Service
    // which might not have created the other (dart) end of the channel and so such channel would
    // not be usable.
    val sharedChannel: MethodChannel?
      get() = synchronized(channels) {
        channels.firstOrNull()
      }

    private fun enableSharingForChannel(channel: MethodChannel) = synchronized(channels) {
      channels.add(channel)
    }

    private fun disableSharingForChannel(channel: MethodChannel) = synchronized(channels) {
      channels.remove(channel)
    }
  }

  override fun onAttachedToActivity(binding: ActivityPluginBinding) {
    Log.d(TAG, "onAttachedToActivity");
    activity = binding.activity

    channel?.let {
      enableSharingForChannel(it)
    }
  }

  override fun onDetachedFromActivity() {
    Log.d(TAG, "onDetachedFromActivity");
    activity = null

    channel?.let {
      disableSharingForChannel(it)
    }

    // TODO: We are no longer using `flutter_background` and this method is never called on app
    // termination (e.g., when the user swipes off all its activities). Figure out how to handle
    // this. Original comment follows:
    //
    // When the user requests for the app to manage it's own battery
    // optimization permissions (e.g. when using the `flutter_background`
    // plugin https://pub.dev/packages/flutter_background), then killing the
    // app will not stop the native code execution and we have to do it
    // manually.
    channel?.invokeMethod("stopSession", null)
  }

  override fun onDetachedFromActivityForConfigChanges() {
    onDetachedFromActivity()
  }

  override fun onReattachedToActivityForConfigChanges(binding: ActivityPluginBinding) {
    onAttachedToActivity(binding)
  }

  override fun onAttachedToEngine(binding: FlutterPlugin.FlutterPluginBinding) {
    Log.d(TAG, "onAttachedToEngine");

    channel = MethodChannel(binding.binaryMessenger, CHANNEL_NAME).also {
      it.setMethodCallHandler(this)

      if (activity != null) {
        enableSharingForChannel(it)
      }
    }
  }

  override fun onDetachedFromEngine(binding: FlutterPlugin.FlutterPluginBinding) {
    Log.d(TAG, "onDetachedFromEngine");

    channel?.let {
      disableSharingForChannel(it)
      it.setMethodCallHandler(null)
    }

    channel = null
  }

  override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
    when (call.method) {
      "shareFile" -> {
        val arguments = call.arguments as HashMap<String, Any>
        startFileShareAction(arguments)
        result.success("Share file intent started")
      }
      "previewFile" -> {
        val arguments = call.arguments as HashMap<String, Any>
        val previewResult = startFilePreviewAction(arguments)
        result.success(previewResult)
      }
      "getDownloadPath" -> {
        val downloadPath = startGetDownloadPath()
        result.success(downloadPath)
      }
      else -> {
        result.notImplemented()
      }
    }
  }

  private fun startGetDownloadPath(): String? {
    val downloadDirectory = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS)
    return downloadDirectory.toString()
  } 

  private fun startFilePreviewAction(arguments: HashMap<String, Any>): String {
    val authority = arguments["authority"]
    val path = arguments["path"] as String
    val size = arguments["size"]
    val useDefaultApp = arguments["useDefaultApp"] as Boolean? ?: false

    val uri = Uri.parse("content://$authority.pipe/$size$path")
    val mimeType = URLConnection.guessContentTypeFromName(path) ?: "application/octet-stream"

    val intent = Intent(Intent.ACTION_VIEW)
      // Some apps (e.g., Google Files) can't open the file unless we specify the type explicitly.
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

  private fun startFileShareAction(arguments: HashMap<String, Any>) {
    val authority = arguments["authority"]
    val path = arguments["path"]
    val size = arguments["size"]
    val title = "Share file from Ouisync"

    val uri = Uri.parse("content://$authority.pipe/$size$path")

    Log.d(javaClass.simpleName, "Uri: ${uri.toString()}")

    val intent = Intent(Intent.ACTION_SEND)
      .setType("*/*")
      .putExtra(Intent.EXTRA_STREAM, uri)
      .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)

    activity?.startActivity(Intent.createChooser(intent, title))
  }
}

