import Foundation
import OuisyncLibFFI

// ErrorCode from cbindgen is typedef uint16_t; Swift imports it as UInt16.
// The callback pointer type used by both start_service and stop_service.
private typealias ServiceCallback = @convention(c) (UnsafeRawPointer?, UInt16) -> Void

// MARK: - OuisyncService

public class OuisyncService {
    let handle: UnsafeMutableRawPointer

    private init(_ handle: UnsafeMutableRawPointer) {
        self.handle = handle
    }

    /// Initialize logging. Call before start().
    public static func initLog() {
        init_log()
    }

    /// Start the ouisync service and wait until it is ready to accept connections.
    /// Returns a service handle; call stop() when done.
    public static func start(configDir: String, debugLabel: String? = nil) async throws -> OuisyncService {
        var rawHandle: UnsafeMutableRawPointer?

        do {
            try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
                let ctx = Unmanaged.passRetained(StartBox(cont)).toOpaque()
                rawHandle = configDir.withCString { cdir in
                    if let label = debugLabel {
                        return label.withCString { clab in
                            start_service(cdir, clab, serviceStarted, ctx)
                        }
                    } else {
                        return start_service(cdir, nil, serviceStarted, ctx)
                    }
                }
            }
        } catch {
            // start_service always returns a handle (the oneshot::Sender box).
            // Free it even when initialization fails; the callback won't fire.
            if let h = rawHandle {
                stop_service(h, serviceNoop, nil)
            }
            throw error
        }

        guard let h = rawHandle else {
            throw OuisyncError(.other, "start_service returned null")
        }
        return OuisyncService(h)
    }

    /// Stop the service and wait for shutdown to complete.
    public func stop() async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            let ctx = Unmanaged.passRetained(StopBox(cont)).toOpaque()
            stop_service(handle, serviceStopped, ctx)
        }
    }
}

// MARK: - Continuation boxes

private class StartBox {
    let continuation: CheckedContinuation<Void, Error>
    init(_ c: CheckedContinuation<Void, Error>) { continuation = c }
}

private class StopBox {
    let continuation: CheckedContinuation<Void, Error>
    init(_ c: CheckedContinuation<Void, Error>) { continuation = c }
}

// MARK: - C callbacks (no captures — compatible with @convention(c))

private func serviceStarted(_ ctx: UnsafeRawPointer?, _ code: UInt16) {
    guard let ctx else { return }
    Unmanaged<StartBox>.fromOpaque(ctx).takeRetainedValue()
        .continuation.resumeWith(result: serviceResult(code))
}

private func serviceStopped(_ ctx: UnsafeRawPointer?, _ code: UInt16) {
    guard let ctx else { return }
    Unmanaged<StopBox>.fromOpaque(ctx).takeRetainedValue()
        .continuation.resumeWith(result: serviceResult(code))
}

private func serviceNoop(_: UnsafeRawPointer?, _: UInt16) {}

private func serviceResult(_ code: UInt16) -> Result<Void, Error> {
    if code == 0 {
        return .success(())
    }
    let errorCode = ErrorCode(rawValue: code) ?? .other
    return .failure(OuisyncError(errorCode, "service error (code \(code))"))
}
