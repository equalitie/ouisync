import OuisyncLibFFI

extension ErrorCode: Swift.Error {}
public typealias Error = ErrorCode

public class Service {
    // An opaque handle which must be passed to service_stop in order to terminate the service.
    private var handle: UnsafeMutableRawPointer?;

    /* Starts a Ouisync service in a new thread and binds it to the port set in `configDir`.
     *
     * Returns after the service has been initialized successfully and is ready to accept client
     * connections, or throws a `Ouisync.Error` indicating what went wrong.
     *
     * On success, the service remains active until `.stop()` is called. An attempt will be made to
     * stop the service once all references are dropped, however this is strongly discouraged as in
     * this case it's not possible to determine whether the shutdown was successful. */
    public init(configDir: String, debugLabel: String) async throws {
        handle = try await withUnsafeThrowingContinuation {
            service_start(configDir, debugLabel, Resume, unsafeBitCast($0, to: UnsafeRawPointer.self))
        }
    }

    deinit {
        guard let handle else { return }
        service_stop(handle, Ignore, nil)
    }

    /* Stops a running Ouisync service.
     *
     * Returns after the service shutdown has been completed or throws a `Ouisync.Error` on failure.
     * Returns immediately when called a second time, but doing so is not thread safe! */
    public func stop() async throws {
        guard let handle else { return }
        self.handle = nil
        try await withUnsafeThrowingContinuation {
            service_stop(handle, Resume, unsafeBitCast($0, to: UnsafeRawPointer.self))
        } as Void
    }

    /* Initialize logging to stdout. Should be called before `service_start`.
     *
     * If `filename` is not null, additionally logs to that file.
     * If `handler` is not null, it is called for every message.
     *
     * Throws a `Ouisync.Error` on failure. Should not be called more than once per process!
     */
    public static func configureLogging(filename: String? = nil,
                                        handler: LogHandler? = nil,
                                        tag: String = "Server") throws {
        let err = log_init(filename, handler, tag)
        if err != .Ok { throw err }
    }
    public typealias LogHandler = @convention(c) (LogLevel, UnsafePointer<CChar>?) -> Void
}

// ffi callbacks
fileprivate func Resume(context: UnsafeRawPointer?, error: Error) {
    let continuation = unsafeBitCast(context, to: UnsafeContinuation<Void, any Swift.Error>.self)
    switch error {
    case .Ok: continuation.resume()
    default: continuation.resume(throwing: error)
    }
}
fileprivate func Ignore(context: UnsafeRawPointer?, error: Error) {}
