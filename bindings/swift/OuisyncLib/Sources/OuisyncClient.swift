//
//  File.swift
//  
//
//  Created by Peter Jankuliak on 23/07/2024.
//

import Foundation
import OuisyncLibFFI

public class OuisyncClient {
    var clientHandle: SessionHandle
    let ffi: OuisyncFFI
    public var onReceiveFromBackend: OuisyncOnReceiveFromBackend? = nil

    public static func create(_ configPath: String, _ logPath: String, _ ffi: OuisyncFFI) throws -> OuisyncClient {
        // Init with an invalid sessionHandle because we need the OuisyncSession instance to
        // create the callback, which is in turn needed to create the proper sessionHandle.
        let client = OuisyncClient(0, ffi)

        let logTag = "ouisync-backend"
        let result = ffi.ffiSessionCreate(ffi.sessionKindShared, configPath, logPath, logTag,
                                          .init(mutating: OuisyncFFI.toUnretainedPtr(obj: client))) {
            context, dataPointer, size in
            let client: OuisyncClient = OuisyncFFI.fromUnretainedPtr(ptr: context!)
            guard let onReceive = client.onReceiveFromBackend else {
                fatalError("OuisyncClient has no onReceive handler set")
            }
            onReceive(Array(UnsafeBufferPointer(start: dataPointer, count: Int(exactly: size)!)))
        }

        if result.error_code != 0 {
            throw SessionCreateError("Failed to create session, code:\(result.error_code), message:\(result.error_message!)")
        }

        client.clientHandle = result.session
        return client
    }

    fileprivate init(_ clientHandle: SessionHandle, _ ffi: OuisyncFFI) {
        self.clientHandle = clientHandle
        self.ffi = ffi
    }

    public func sendToBackend(_ data: [UInt8]) {
        let count = data.count;
        data.withUnsafeBufferPointer({ maybePointer in
            if let pointer = maybePointer.baseAddress {
                ffi.ffiSessionChannelSend(clientHandle, .init(mutating: pointer), UInt64(count))
            }
        })
    }

    public func close() async {
        typealias Continuation = CheckedContinuation<Void, Never>

        class Context {
            let clientHandle: SessionHandle
            let continuation: Continuation
            init(_ clientHandle: SessionHandle, _ continuation: Continuation) {
                self.clientHandle = clientHandle
                self.continuation = continuation
            }
        }

        await withCheckedContinuation(function: "FFI.closeSession", { continuation in
            let context = OuisyncFFI.toRetainedPtr(obj: Context(clientHandle, continuation))
            let callback: FFICallback = { context, dataPointer, size in
                let context: Context = OuisyncFFI.fromRetainedPtr(ptr: context!)
                context.continuation.resume()
            }
            ffi.ffiSessionClose(clientHandle, .init(mutating: context), callback)
        })
    }
}

public typealias OuisyncOnReceiveFromBackend = ([UInt8]) -> Void
