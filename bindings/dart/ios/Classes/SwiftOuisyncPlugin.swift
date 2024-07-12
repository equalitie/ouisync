import Flutter
import UIKit

public class SwiftOuisyncPlugin: NSObject, FlutterPlugin {
  public static func register(with registrar: FlutterPluginRegistrar) {
    let channel = FlutterMethodChannel(name: "ouisync", binaryMessenger: registrar.messenger())
    let instance = SwiftOuisyncPlugin()
    registrar.addMethodCallDelegate(instance, channel: channel)
  }

  public func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
    result("iOS " + UIDevice.current.systemVersion)
  }

  /// This method is necessary to prevent the library symbols to be stripped when compiling the IPA
  /// More info here: https://stackoverflow.com/questions/71133823/problems-publishing-a-flutter-app-with-a-binded-rust-binary-to-the-appstore
  ///    - https://github.com/fzyzcjy/flutter_rust_bridge/issues/496
  ///    - https://cjycode.com/flutter_rust_bridge/integrate/ios_headers.html
  ///
  /// The key is to dummy-call and use the result on every function (for debug, only one was enough)
  public func dummyMethodToEnforceBundling() {
    // This will never be executed
    let context: UnsafeMutableRawPointer? = nil
    let callback: Callback? = nil

    let sessionKind: UInt8 = 0

    let resultCreateC = session_create(sessionKind, "", "", context, callback)
    print(resultCreateC)    

    let function: PostDartCObjectFn? = nil
    let port: Int64 = 0

    let resultCreate = session_create_dart(sessionKind, "", "", function, port)
    print(resultCreate)

    let session: SessionHandle = 0

    let resultCloseC = session_close(session, context, callback)
    print(resultCloseC)

    let resultClose = session_close_dart(session, function, port)
    print(resultClose)

    let resultCloseBlocking = session_close_blocking(session)
    print(resultCloseBlocking)

    let payload: UnsafeMutablePointer<UInt8>? = nil
    let length: UInt64 = 0
    
    let resultSend = session_channel_send(session, payload, length)
    print(resultSend)

    let handle: UInt64 = 0

    let resultFileCopy = file_copy_to_raw_fd_dart(session, handle, 0, function, port)
    print(resultFileCopy)

    let resultFileCopyNS = file_copy_to_raw_fd_dart(session, handle, 0, function, port)
    print(resultFileCopyNS)
    
    let stringPointer: UnsafeMutablePointer<Int>? = nil

    let resultFreeString = free_string(stringPointer)
    print(resultFreeString)

    let level: UInt8 = 0

    let resultLogPrint = log_print(level, "", "")
    print(resultLogPrint)
  }
}
