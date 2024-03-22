package ie.equalit.ouisync_plugin

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.util.Log
import android.webkit.MimeTypeMap
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

  companion object {
    private val TAG = OuisyncPlugin::class.java.simpleName

    private const val CHANNEL_NAME = "ouisync_plugin"

    /// The channel needs to be static so we can access it from `PipeProvider` but we need to make
    /// sure we only create/destroy it once even when there are multiple `OuisyncPlugin` instances.
    /// That's why we use the explicit ref count.
    private var channel : MethodChannel? = null
    private val channelLock = Any()
    private var channelRefCount = 0
    private var isHandlingIntent = false

    fun invokeMethod(method: String, arguments: Any?, callback: MethodChannel.Result? = null) {
      channel.let {
        if (it != null) {
          it.invokeMethod(method, arguments, callback)
        } else {
          callback?.error("not attached to engine", null, null)
        }
      }
    }
  }

  override fun onAttachedToActivity(activityPluginBinding: ActivityPluginBinding) {
    Log.d(TAG, "onAttachedToActivity");

    isHandlingIntent = true
    activity = activityPluginBinding.activity
  }

  override fun onDetachedFromActivityForConfigChanges() {
    Log.d(TAG, "onDetachedFromActivityForConfigChanges");
    activity = null
  }

  override fun onReattachedToActivityForConfigChanges(activityPluginBinding: ActivityPluginBinding) {
    Log.d(TAG, "onReattachedToActivityForConfigChanges");
    activity = activityPluginBinding.activity
  }

  override fun onDetachedFromActivity() {
    Log.d(TAG, "onDetachedFromActivity");
    activity = null
  }

  override fun onAttachedToEngine(flutterPluginBinding: FlutterPlugin.FlutterPluginBinding) {
    Log.d(TAG, "onAttachedToEngine");

    synchronized(channelLock) {
      channelRefCount++

      if (channelRefCount == 1) {
        Log.d(TAG, "create method channel")

        channel = MethodChannel(flutterPluginBinding.binaryMessenger, CHANNEL_NAME)
        channel?.setMethodCallHandler(this)
      }
    }
  }

  override fun onDetachedFromEngine(binding: FlutterPlugin.FlutterPluginBinding) {
    Log.d(TAG, "onDetachedFromEngine");

    Log.d(TAG, "If we are in a new activity, most likely it is for handling a share intent from a different task/process. We can't stop the session in the original task")
    if (!isHandlingIntent) {
      // When the user requests for the app to manage it's own battery
      // optimization permissions (e.g. when using the `flutter_background`
      // plugin https://pub.dev/packages/flutter_background), then killing the
      // app will not stop the native code execution and we have to do it
      // manually.
      Log.d(TAG, "onDetachedFromEngine: closing Ouisync session");
      invokeMethod("stopSession", null)
    } else isHandlingIntent = false

    synchronized(channelLock) {
      channelRefCount--

      if (channelRefCount == 0) {
        Log.d(TAG, "destroy method channel")

        channel?.setMethodCallHandler(null)
      }
    }
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
      else -> {
        result.notImplemented()
      }
    }
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
}
