//
//  File.swift
//  
//
//  Created by Peter Jankuliak on 19/07/2024.
//

import Foundation
import OuisyncLibFFI


/* TODO: ⬇️

 Since we're now linking statically and both rust-cbindgen and swift do a reasonable job at guessing
 the intended types, I don't expect these types to ever make it to the main branch because this file
 will most likely go away. For now they are kept to avoid touching too much code in a single commit.
 */
typealias FFISessionKind = UInt8 // swift gets confused here and imports a UInt32 enum as well as a UInt8 typealias
typealias FFIContext = UnsafeMutableRawPointer // exported as `* mut ()` in rust so this is correct, annoyingly
typealias FFICallback = @convention(c) (FFIContext?, UnsafePointer<UInt8>?, UInt64) -> Void;
typealias FFISessionCreate = @convention(c) (FFISessionKind, UnsafePointer<Int8>?, UnsafePointer<Int8>?, UnsafePointer<Int8>?, FFIContext?, FFICallback?) -> SessionCreateResult;
typealias FFISessionGrab = @convention(c) (FFIContext?, FFICallback?) -> SessionCreateResult;
typealias FFISessionClose = @convention(c) (SessionHandle, FFIContext?, FFICallback?) -> Void;
typealias FFISessionChannelSend = @convention(c) (SessionHandle, UnsafeMutablePointer<UInt8>?, UInt64) -> Void;

class SessionCreateError : Error, CustomStringConvertible {
    let message: String
    init(_ message: String) { self.message = message }
    var description: String { message }
}

public class OuisyncFFI {
    // let handle: UnsafeMutableRawPointer
    let ffiSessionGrab: FFISessionGrab
    let ffiSessionCreate: FFISessionCreate
    let ffiSessionChannelSend: FFISessionChannelSend
    let ffiSessionClose: FFISessionClose
    let sessionKindShared: FFISessionKind = 0;

    public init() {
        // The .dylib is created using the OuisyncDyLibBuilder package plugin in this Swift package.
        // let libraryName = "libouisync_ffi.dylib"
        // let resourcePath = Bundle.main.resourcePath! + "/OuisyncLib_OuisyncLibFFI.bundle/Contents/Resources"
        // handle = dlopen("\(resourcePath)/\(libraryName)", RTLD_NOW)!

        ffiSessionGrab = session_grab
        ffiSessionChannelSend = session_channel_send
        ffiSessionClose = session_close
        ffiSessionCreate = session_create

        //ffiSessionGrab = unsafeBitCast(dlsym(handle, "session_grab"), to: FFISessionGrab.self)
        //ffiSessionChannelSend = unsafeBitCast(dlsym(handle, "session_channel_send"), to: FFISessionChannelSend.self)
        //ffiSessionClose = unsafeBitCast(dlsym(handle, "session_close"), to: FFISessionClose.self)
        //ffiSessionCreate = unsafeBitCast(dlsym(handle, "session_create"), to: FFISessionCreate.self)
    }

    // Blocks until Dart creates a session, then returns it.
    func waitForSession(_ context: FFIContext, _ callback: FFICallback) async throws -> SessionHandle {
        // TODO: Might be worth change the ffi function to call a callback when the session becomes created instead of bussy sleeping.
        var elapsed: UInt64 = 0;
        while true {
            let result = ffiSessionGrab(context, callback)
            if result.error_code == 0 {
                NSLog("😀 Got Ouisync session");
                return result.session
            }
            NSLog("🤨 Ouisync session not yet ready. Code: \(result.error_code) Message:\(String(cString: result.error_message!))");

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
