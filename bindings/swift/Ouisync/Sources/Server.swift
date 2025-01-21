import CryptoKit
import Foundation
import OuisyncService

extension ErrorCode: Error {} // @retroactive doesn't work in Ventura, which I still use
public typealias OuisyncError = ErrorCode

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
        self.configDir = URL(fileURLWithPath: configDir)
        self.debugLabel = debugLabel
        handle = try await withUnsafeThrowingContinuation {
            service_start(configDir, debugLabel, Resume, unsafeBitCast($0, to: UnsafeRawPointer.self))
        }
    }
    /// the configDir passed to the constructor when the server was started
    public let configDir: URL
    /// the debugLabel passed to the constructor when the server was started
    public let debugLabel: String
    /// The localhost port that can be used to interact with the server
    public var port: UInt16 { get async throws { try JSONDecoder().decode(UInt16.self, from:
        Data(contentsOf: configDir.appending(component: "local_control_port.conf")))
    } }
    /// The HMAC key required to authenticate to the server listening on `port`
    public var authKey: SymmetricKey { get async throws {
        let file = configDir.appending(component: "local_control_auth_key.conf")
        let str = try JSONDecoder().decode(String.self, from: Data(contentsOf: file))
        guard str.count&1 == 0 else { throw CryptoKitError.incorrectParameterSize }

        // unfortunately, swift doesn't provide an (easy) way to do hex decoding
        var curr = str.startIndex
        return try SymmetricKey(data: (0..<str.count>>1).map { _ in
            let next = str.index(after: curr)
            defer { curr = next }
            if let res = UInt8(str[curr...next], radix: 16) { return res }
            throw CryptoKitError.invalidParameter
        })
    } }

    /// An opaque handle which must be passed to `service_stop` in order to terminate the service.
    private var handle: UnsafeMutableRawPointer?;
    deinit {
        guard let handle else { return }
        service_stop(handle, Ignore, nil)
    }

    /** Stops a running Ouisync server.
     *
     * Returns after the server shutdown has been completed or throws a `OuisyncError` on failure.
     * Returns immediately when called a second time, but doing so is not thread safe! */
    public func stop() async throws {
        guard let handle else { return }
        self.handle = nil
        try await withUnsafeThrowingContinuation {
            service_stop(handle, Resume, unsafeBitCast($0, to: UnsafeRawPointer.self))
        } as Void
    }

    /** Opens a new client connection to this server. */
    public func connect() async throws -> Client { try await .init(port, authKey) }

    /** Initialize logging to stdout. Should be called before constructing a `Server`.
     *
     * If `filename` is not null, additionally logs to that file.
     * If `handler` is not null, it is called for every message.
     *
     * Throws a `OuisyncError` on failure. Should not be called more than once per process!
     */
    public static func configureLogging(filename: String? = nil,
                                        handler: LogHandler? = nil,
                                        tag: String = "Server") throws {
        let err = log_init(filename, handler, tag)
        if err != .Ok { throw err }
    }
    public typealias LogHandler = @convention(c) (LogLevel, UnsafePointer<CChar>?) -> Void
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
