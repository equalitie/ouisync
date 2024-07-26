//
//  File.swift
//  
//
//  Created by Peter Jankuliak on 19/07/2024.
//

import Foundation
import OuisyncLibFFI

typealias FFISessionKind = UInt8
typealias FFIContext = UnsafeRawPointer
typealias FFICallback = @convention(c) (FFIContext?, UnsafePointer<UInt8>, CUnsignedLongLong) -> Void;
typealias FFISessionCreate = @convention(c) (FFISessionKind, UnsafePointer<UInt8>, UnsafePointer<UInt8>, UnsafeRawPointer?, FFICallback) -> OuisyncSessionCreateResult;
typealias FFISessionGrab = @convention(c) (UnsafeRawPointer?, FFICallback) -> OuisyncSessionCreateResult;
typealias FFISessionClose = @convention(c) (OuisyncClientHandle, FFIContext?, FFICallback) -> Void;
typealias FFISessionChannelSend = @convention(c) (OuisyncClientHandle, UnsafeRawPointer, UInt64) -> Void;

class SessionCreateError : Error, CustomStringConvertible {
    let message: String
    init(_ message: String) { self.message = message }
    var description: String { message }
}

public class OuisyncFFI {
    let handle: UnsafeMutableRawPointer
    let ffiSessionGrab: FFISessionGrab
    let ffiSessionCreate: FFISessionCreate
    let ffiSessionChannelSend: FFISessionChannelSend
    let ffiSessionClose: FFISessionClose
    let sessionKindShared: FFISessionKind = 0;

    public init() {
        // The .dylib is created using the OuisyncDyLibBuilder package plugin in this Swift package.
        let libraryName = "libouisync_ffi.dylib"
        let resourcePath = Bundle.main.resourcePath! + "/OuisyncLib_OuisyncLibFFI.bundle/Contents/Resources"
        handle = dlopen("\(resourcePath)/\(libraryName)", RTLD_NOW)!

        ffiSessionGrab = unsafeBitCast(dlsym(handle, "session_grab"), to: FFISessionGrab.self)
        ffiSessionChannelSend = unsafeBitCast(dlsym(handle, "session_channel_send"), to: FFISessionChannelSend.self)
        ffiSessionClose = unsafeBitCast(dlsym(handle, "session_close"), to: FFISessionClose.self)
        ffiSessionCreate = unsafeBitCast(dlsym(handle, "session_create"), to: FFISessionCreate.self)
    }

    // Blocks until Dart creates a session, then returns it.
    func waitForSession(_ context: UnsafeRawPointer, _ callback: FFICallback) async throws -> OuisyncClientHandle {
        // TODO: Might be worth change the ffi function to call a callback when the session becomes created instead of bussy sleeping.
        var elapsed: UInt64 = 0;
        while true {
            let result = ffiSessionGrab(context, callback)
            if result.errorCode == 0 {
                NSLog("😀 Got Ouisync session");
                return result.clientHandle
            }
            NSLog("🤨 Ouisync session not yet ready. Code: \(result.errorCode) Message:\(String(cString: result.errorMessage!))");

            let millisecond: UInt64 = 1_000_000
            let second: UInt64 = 1000 * millisecond

            var timeout = 200 * millisecond

            if elapsed > 10 * second {
                timeout = second
            }

            try await Task.sleep(nanoseconds: timeout)
            elapsed += timeout;
        }
    }

    // Retained pointers have their reference counter incremented by 1.
    // https://stackoverflow.com/a/33310021/273348
    static func toUnretainedPtr<T : AnyObject>(obj : T) -> UnsafeRawPointer {
        return UnsafeRawPointer(Unmanaged.passUnretained(obj).toOpaque())
    }

    static func fromUnretainedPtr<T : AnyObject>(ptr : UnsafeRawPointer) -> T {
        return Unmanaged<T>.fromOpaque(ptr).takeUnretainedValue()
    }

    static func toRetainedPtr<T : AnyObject>(obj : T) -> UnsafeRawPointer {
        return UnsafeRawPointer(Unmanaged.passRetained(obj).toOpaque())
    }

    static func fromRetainedPtr<T : AnyObject>(ptr : UnsafeRawPointer) -> T {
        return Unmanaged<T>.fromOpaque(ptr).takeRetainedValue()
    }
}
