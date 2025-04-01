import CryptoKit
import Foundation
import OuisyncService


// @retroactive doesn't work in Ventura, which I still use
extension ErrorCode: Error, CustomDebugStringConvertible {
    public var debugDescription: String { "OuisyncError(code=\(rawValue))" }
}
public typealias OuisyncError = ErrorCode


// FIXME: updating this at runtime is unsafe and should be cast to atomic
public var ouisyncLogHandler: ((LogLevel, String) -> Void)?

// init_log is not safe to call repeatedly, should only be called before the first server is
// started and provides awkward memory semantics to assist dart, though we may eventually end up
// using them here as well if ever we end up making logging async
private func directLogHandler(_ level: LogLevel, _ ptr: UnsafePointer<UInt8>?, _ len: UInt, _ cap: UInt) {
    defer { release_log_message(ptr, len, cap) }
    if let ptr { ouisyncLogHandler?(level, String(decoding: UnsafeBufferPointer(start: ptr,
                                                                                count: Int(len)),
                                                  as: UTF8.self)) }
}
@MainActor private var loggingConfigured = false
@MainActor private func setupLogging() async throws {
    if loggingConfigured { return }
    loggingConfigured = true
    let err = init_log(nil, directLogHandler)
    guard case .Ok = err else { throw err }
}


public class Server {
    /** Starts a Ouisync server in a new thread and binds it to the port set in `configDir`.
     *
     * Returns after the socket has been initialized successfully and is ready to accept client
     * connections, or throws a `OuisyncError` indicating what went wrong.
     *
     * On success, the server remains active until `.stop()` is called. An attempt will be made to
     * stop the server once all references are dropped, however this is strongly discouraged since
     * in this case it's not possible to determine whether the shutdown was successful or not. */
    public init(configDir: String, debugLabel: String) async throws {
        try await setupLogging()
        self.configDir = configDir
        self.debugLabel = debugLabel
        try await withUnsafeThrowingContinuation {
            handle = start_service(configDir, debugLabel, Resume, unsafeBitCast($0, to: UnsafeRawPointer.self))
        } as Void
    }
    /// the `configDir` passed to the constructor when the server was started
    public let configDir: String
    /// the `debugLabel` passed to the constructor when the server was started
    public let debugLabel: String
    /// The localhost `port` that can be used to interact with the server (IPv4)
    public var port: UInt16 { get async throws { try JSONDecoder().decode(UInt16.self, from:
        Data(contentsOf: URL(fileURLWithPath: configDir.appending("/local_control_port.conf"))))
    } }
    /// The HMAC key required to authenticate to the server listening on `port`
    public var authKey: SymmetricKey { get async throws {
        let file = URL(fileURLWithPath: configDir.appending("/local_control_auth_key.conf"))
        let str = try JSONDecoder().decode(String.self, from: Data(contentsOf: file))
        guard str.count&1 == 0 else { throw CryptoKitError.incorrectParameterSize }

        // unfortunately, swift doesn't provide an (easy) way to do hex decoding
        var curr = str.startIndex
        return try SymmetricKey(data: (0..<str.count>>1).map { _ in
            let next = str.index(curr, offsetBy: 2)
            defer { curr = next }
            if let res = UInt8(str[curr..<next], radix: 16) { return res }
            throw CryptoKitError.invalidParameter
        })
    } }

    /// An opaque handle which must be passed to `service_stop` in order to terminate the service.
    private var handle: UnsafeMutableRawPointer?;
    deinit {
        guard let handle else { return }
        stop_service(handle, Ignore, nil)
    }

    /** Stops a running Ouisync server.
     *
     * Returns after the server shutdown has been completed or throws a `OuisyncError` on failure.
     * Returns immediately when called a second time, but doing so is not thread safe! */
    public func stop() async throws {
        guard let handle else { return }
        self.handle = nil
        try await withUnsafeThrowingContinuation {
            stop_service(handle, Resume, unsafeBitCast($0, to: UnsafeRawPointer.self))
        } as Void
    }

    /** Opens a new client connection to this server. */
    public func connect() async throws -> Client { try await .init(port, authKey) }
}

/// FFI callback that expects a continuation in the context which it resumes, throwing if necessary
fileprivate func Resume(context: UnsafeRawPointer?, error: OuisyncError) {
    let continuation = unsafeBitCast(context, to: UnsafeContinuation<Void, any Error>.self)
    switch error {
    case .Ok: continuation.resume()
    default: continuation.resume(throwing: error)
    }
}
/// FFI callback that does nothing; can be removed if upstream allows null function pointers
fileprivate func Ignore(context: UnsafeRawPointer?, error: OuisyncError) {}
