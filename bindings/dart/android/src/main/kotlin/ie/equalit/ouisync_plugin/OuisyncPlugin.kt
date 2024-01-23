package ie.equalit.ouisync_plugin

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.util.Log
import android.os.Environment
import android.webkit.MimeTypeMap
import androidx.annotation.NonNull
import io.flutter.embedding.engine.plugins.FlutterPlugin
import io.flutter.embedding.engine.plugins.activity.ActivityAware
import io.flutter.embedding.engine.plugins.activity.ActivityPluginBinding
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.MethodChannel.MethodCallHandler
import io.flutter.plugin.common.MethodChannel.Result

/** OuisyncPlugin */
class OuisyncPlugin: FlutterPlugin, MethodCallHandler, ActivityAware {
  /// The MethodChannel that will the communication between Flutter and native Android
  ///
  /// This local reference serves to register the plugin with the Flutter Engine and unregister it
  /// when the Flutter Engine is detached from the Activity
  var activity : Activity? = null
  private lateinit var context : Context

  companion object {
    lateinit var channel : MethodChannel
  }

  override fun onAttachedToActivity(@NonNull activityPluginBinding: ActivityPluginBinding) {
    print("onAttachedToActivity")
    activity = activityPluginBinding.activity
  }

  override fun onDetachedFromActivityForConfigChanges() {
    activity = null
  }

  override fun onReattachedToActivityForConfigChanges(@NonNull activityPluginBinding: ActivityPluginBinding) {
    activity = activityPluginBinding.activity
  }

  override fun onDetachedFromActivity() {
    activity = null
  }

  override fun onAttachedToEngine(@NonNull flutterPluginBinding: FlutterPlugin.FlutterPluginBinding) {
    channel = MethodChannel(flutterPluginBinding.binaryMessenger, "ouisync_plugin")
    channel.setMethodCallHandler(this)

    context = flutterPluginBinding.getApplicationContext()
  }

  override fun onMethodCall(@NonNull call: MethodCall, @NonNull result: MethodChannel.Result) {

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
    val path = arguments["path"]
    val size = arguments["size"]
    val useDefaultApp = arguments["useDefaultApp"]

    val extension = getFileExtension(path as String)
    val mimeType = getMimeType(extension)

    if (mimeType == null) {
      Log.d(javaClass.simpleName, """PreviewFile: No mime type was found for the file extension $extension
        We can't determine the default app for the file $path""")

      return "mimeTypeNull"
    }

    Log.d(javaClass.simpleName, "File extension: $extension")
    Log.d(javaClass.simpleName, "Mime type: $mimeType")

    val uri = Uri.parse("content://$authority.pipe/$size$path")
    Log.d(javaClass.simpleName, "Uri: ${uri.toString()}")

    val intent = getIntentForAction(uri, Intent.ACTION_VIEW)

    try {
        if (useDefaultApp != null) {
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

  private fun getMimeType(extension: String): String? {
    return MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension)
  }

  private fun getFileExtension(path: String): String {
    return MimeTypeMap.getFileExtensionFromUrl(path)
  }

  private fun startFileShareAction(arguments: HashMap<String, Any>) {
    val authority = arguments["authority"]
    val path = arguments["path"]
    val size = arguments["size"]
    val title = "Share file from Ouisync"

    val uri = Uri.parse("content://$authority.pipe/$size$path")

    Log.d(javaClass.simpleName, "Uri: ${uri.toString()}")

    val intent = getIntentForAction(uri, Intent.ACTION_SEND)
    intent.setType("*/*")

    activity?.startActivity(Intent.createChooser(intent, title))
  }

  private fun getIntentForAction(
          intentData: Uri,
          intentAction: String
  ) = Intent().apply {
        data = intentData
        action = intentAction

        putExtra(Intent.EXTRA_STREAM, intentData)
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
      }

  override fun onDetachedFromEngine(@NonNull binding: FlutterPlugin.FlutterPluginBinding) {
    // When the user requests for the app to manage it's own battery
    // optimization permissions (e.g. when using the `flutter_background`
    // plugin https://pub.dev/packages/flutter_background), then killing the
    // app will not stop the native code execution and we have to do it
    // manually.
    channel.invokeMethod("stopSession", null)
    channel.setMethodCallHandler(null)
  }
}
