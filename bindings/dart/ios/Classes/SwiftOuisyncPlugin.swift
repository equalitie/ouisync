import Flutter
import UIKit

public class SwiftOuisyncPlugin: NSObject, FlutterPlugin {
  public static func register(with registrar: FlutterPluginRegistrar) {
    let channel = FlutterMethodChannel(name: "ouisync_plugin", binaryMessenger: registrar.messenger())
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

    let session: SessionHandle = 0
    let port: Int64 = 0
    let pointer: Int64 = 0
    let function: PostDartCObjectFn? = nil
    let payload: UnsafeMutablePointer<UInt8>? = nil
    let length: UInt64 = 0
    let handle: UInt64 = 0
    let stringPointer: UnsafeMutablePointer<Int>? = nil
    let level: UInt8 = 0

    let resultCreate = session_create_dart("", "", function, port)
    print(resultCreate)

    let resultClose = session_close(session)
    print(resultClose)
    
    let resultSend = session_channel_send(session, payload, length)
    print(resultSend)

    let resultShutdown = session_shutdown_network_and_close(session)
    print(resultShutdown)

    let resultFileCopy = file_copy_to_raw_fd_dart(session, handle, 0, function, port)
    print(resultFileCopy)

    let resultFileCopy2 = file_copy_to_raw_fd_dart(session, handle, 0, function, port)
    print(resultFileCopy2)

    let resultFreeString = free_string(stringPointer)
    print(resultFreeString)

    let resultLogPrint = log_print(level, "", "")
    print(resultLogPrint)
  }
}
