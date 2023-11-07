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
