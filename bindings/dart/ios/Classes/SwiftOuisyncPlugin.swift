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
    let bytesPointer = UnsafeMutableRawPointer.allocate(byteCount: 4, alignment: 4)
    session_open(bytesPointer,"",0)
  }
}
