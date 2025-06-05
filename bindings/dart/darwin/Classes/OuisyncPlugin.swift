#if os(iOS)
    import Flutter
    import UIKit
    typealias BaseClass = UIView
#else
    import FlutterMacOS
    import Cocoa
    typealias BaseClass = NSView
#endif

public class OuisyncPlugin: NSObject, FlutterPlugin {
  public static func register(with registrar: FlutterPluginRegistrar) {
    #if os(iOS)
      let channel = FlutterMethodChannel(name: "ouisync", binaryMessenger: registrar.messenger())
    #else
      let channel = FlutterMethodChannel(name: "ouisync", binaryMessenger: registrar.messenger)
    #endif
    let instance = OuisyncPlugin()
    registrar.addMethodCallDelegate(instance, channel: channel)
  }

  public func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
    switch call.method {
    case "getPlatformVersion":
      #if os(iOS)
        result("iOS " + UIDevice.current.systemVersion)
      #else
        result("macOS " + ProcessInfo.processInfo.operatingSystemVersionString)
      #endif
    default:
      result(FlutterMethodNotImplemented)
    }
  }
}
