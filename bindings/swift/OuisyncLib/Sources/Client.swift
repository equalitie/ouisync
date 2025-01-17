import CryptoKit
import Foundation
import MessagePack
import Network


@MainActor public class Client {
    let sock: NWConnection
    let limit: Int
    private(set) var invocations = [UInt64: UnsafeContinuation<MessagePackValue, any Error>]()
    private(set) var subscriptions = [UInt64: Subscription.Continuation]()

    /** Connects to `127.0.0.1:port` and attempts to authenticate the peer using `key`.
     *
     * Throws on connection error or if the peer could not be authenticated. */
    public init(_ port: UInt16, _ key: SymmetricKey, maxMessageSize: Int = 1<<18) async throws {
        limit = maxMessageSize
        sock = NWConnection(to: .hostPort(host: .ipv4(.loopback),
                                          port: .init(rawValue: port)!),
                            using: .tcp)
        sock.start(queue: .main)

        // generate and send client challenge; 256 bytes is a bit large, but we'll probably...
        var clientChallenge = Data.secureRandom(256) // ... reserve portions for non-random headers
        try await send(clientChallenge)

        // receive server challenge and send proof
        let serverChallenge = try await recv(exactly: clientChallenge.count)
        try await send(Data(HMAC<SHA256>.authenticationCode(for: serverChallenge, using: key)))

        // receive and validate server proof
        guard HMAC<SHA256>.isValidAuthenticationCode(try await recv(exactly: SHA256.byteCount),
                                                     authenticating: clientChallenge, using: key)
        else { throw CryptoKitError.authenticationFailure } // early eof or key mismatch

        read() // this keeps calling itself until the socket is closed

        // NWConnection predates concurrency, so we need these to ping-pong during this handshake
        func recv(exactly count: Int) async throws -> Data {
            try await withUnsafeThrowingContinuation { main in
                sock.receive(minimumIncompleteLength: count, maximumLength: count) { data, _, _, err in
                    if let err {
                        main.resume(throwing: err)
                    } else if let data, data.count == count {
                        main.resume(returning: data)
                    } else {
                        return main.resume(throwing: CryptoKitError.authenticationFailure)
                    }
                }
            }
        }
        func send(_ data: any DataProtocol) async throws {
            try await withUnsafeThrowingContinuation { main in
                sock.send(content: data, completion: .contentProcessed({ err in
                    guard let err else { return main.resume() }
                    main.resume(throwing: err)
                }))
            }
        }
    }

    /** Notifies all callers of the intent to gracefully terminate this connection.
     *
     * All active subscriptions are marked as completely consumed
     * All pending remote procedure calls are cancelled with a `CancellationError`
     *
     * The underlying connection is *NOT* closed (in fact it is still actively being used to send
     * any `unsubscribe` messages) and this client can continue be used to send additional requests
     * for as long as there's at least one reference to it.
     *
     * When investigating dangling references in debug mode, you can set the `abortingInDebug`
     * argument to `true` to explicitly abort the connection, but this does not work in release! */
    public func cancel(abortingInDebug: Bool = false) {
        assert({
            if abortingInDebug { abort("User triggered abort for debugging purposes") }
            return true
            // the remaining code is a no-op since we're locked and abort() flushes all state
        }())
        subscriptions.values.forEach { $0.finish() }
        subscriptions.removeAll()
        invocations.values.forEach { $0.resume(throwing: CancellationError()) }
        invocations.removeAll()
    }

    /** Deinitializers are currently synchronous so we can't gracefully unsubscribe, but we can
     * schedule a `RST` packet in order to allow  the server to clean up after this connection */
    deinit { sock.cancel() }

    // MARK: end of public API
    /** This internal function prints `reason` to the standard log, then closes the socket and fails
     * all outstanding requests with `OuisyncError.ConnectionAborted`; intended for use as a generic
     * __panic handler__ whenever a non-recoverable protocol error occurs. */
    private func abort(_ reason: String) {
        print(reason)
        sock.cancel()
        subscriptions.values.forEach { $0.finish(throwing: OuisyncError.ConnectionAborted) }
        subscriptions.removeAll()
        invocations.values.forEach { $0.resume(throwing: OuisyncError.ConnectionAborted) }
        invocations.removeAll()
    }

    /** Runs the remote procedure call `method`, optionally sending `arg` and returns its result.
     *
     * Throws `OuisyncError.ConnectionAborted` if the client is no longer connected to the server.
     * Throws `CancellationError` if the `cancel()` method is called while this call is pending. */
    @discardableResult func invoke(_ method: String,
                                   with arg: MessagePackValue = .nil,
                                   as: UInt64? = nil) async throws -> MessagePackValue {
        guard case .ready = sock.state else { throw OuisyncError.ConnectionAborted }
        return try await withUnsafeThrowingContinuation {
            let id = `as` ?? Self.next()
            let body = pack(arg)
            var message = Data(count: 12)
            message.withUnsafeMutableBytes {
                $0.storeBytes(of: UInt32(exactly: body.count + 8)!.bigEndian, as: UInt32.self)
                $0.storeBytes(of: id, toByteOffset: 4, as: UInt64.self)
            }
            message.append(body)
            invocations[id] = $0
            // TODO: schedule this manually to set an upper limit on memory usage
            sock.send(content: message, completion: .contentProcessed({ err in
                guard let err else { return }
                MainActor.assumeIsolated { self.abort("Unexpected IO error during send: \(err)") }
            }))
        }
    }

    /** Starts a new subscription to `topic`, optionally sending `arg`.
     *
     * Throws `OuisyncError.ConnectionAborted` if the client is no longer connected to the server.
     * Completes normally if the `cancel()` method is called while the subscription is active.
     *
     * The subscription retains the client and until it either goes out of scope. */
    func subscribe(to topic: String, with arg: MessagePackValue = .nil) -> Subscription {
        let id = Self.next()
        let result = Subscription {
            subscriptions[id] = $0
            $0.onTermination = { _ in Task { @MainActor in
                self.subscriptions.removeValue(forKey: id)
                do { try await self.invoke("unsubscribe", with: .uint(id)) }
                catch { print("Unexpected error during unsubscribe: \(error)") }
            } }
        }
        Task {
            do { try await invoke("\(topic)_subscribe", with: arg, as: id) }
            catch { subscriptions.removeValue(forKey: id)?.finish(throwing: error) }
        }
        return result
    }
    public typealias Subscription = AsyncThrowingStream<MessagePackValue, any Error>

    /** Internal function that recursively schedules itself until a permanent error occurs
     *
     * Implemented using callbacks here because while continuations are _cheap_, they are not
     * _free_ and non-main actors are still a bit too thread-hoppy with regards to performance */
    private func read() {
        sock.receive(minimumIncompleteLength: 12, maximumLength: 12) { header, _ , _, err in
            MainActor.assumeIsolated {
                guard err == nil else {
                    return self.abort("Unexpected IO error while reading header: \(err!)")
                }
                guard let header, header.count == 12 else {
                    return self.abort("Unexpected EOF while reading header")
                }

                var size = Int(0)
                var id = UInt64(0)
                header.withUnsafeBytes {
                    size = Int(UInt32(bigEndian: $0.load(as: UInt32.self)))
                    id = $0.load(fromByteOffset: 4, as: UInt64.self)
                }
                guard size <= self.limit else {
                    return self.abort("Received \(size) byte packet which exceeds \(self.limit)")
                }
                self.sock.receive(minimumIncompleteLength: size, maximumLength: size) { body, _, _, err in
                    MainActor.assumeIsolated {
                        guard err == nil else {
                            return self.abort("Unexpected IO error while reading body: \(err!)")
                        }
                        guard let body, header.count == size else {
                            return self.abort("Unexpected EOF while reading body")
                        }
                        guard let (message, rest) = try? unpack(body) else {
                            return self.abort("MessagePack deserialization error")
                        }
                        guard rest.isEmpty else {
                            return self.abort("Received trailing data after MessagePack response")
                        }

                        // TODO: fix message serialization on the rust side to make this simpler
                        let result: Result<MessagePackValue, any Error>
                        guard let payload = message.dictionaryValue else {
                            return self.abort("Received non-dictionary MessagePack response")
                        }
                        if let success = payload["success"] {
                            if success.stringValue != nil {
                                result = .success(.nil)
                            } else if let sub = success.dictionaryValue, sub.count == 1 {
                                result = .success(sub.values.first!)
                            } else {
                                return self.abort("Received unrecognized result: \(success)")
                            }
                        } else if let failure = payload["failure"] {
                            guard let info = failure.arrayValue,
                                  let code = info[0].uint16Value,
                                  let err = OuisyncError(rawValue: code)
                            else { return self.abort("Received unrecognized error: \(failure)") }
                            result = .failure(err)
                        } else { return self.abort("Received unercognized message: \(payload)") }

                        if let callback = self.invocations.removeValue(forKey: id) {
                            callback.resume(with: result)
                        } else if let subscription = self.subscriptions[id] {
                            subscription.yield(with: result)
                        } else {
                            print("Ignoring unexpected message with \(id)")
                        }
                        DispatchQueue.main.async{ self.read() }
                    }
                }
            }
        }
    }

    /** Global message counter; 64 bits are enough that we probably won't run into overflows and
     * having non-reusable values helps with debugging; we also skip 0 because it's ambiguous we
     * could use an atomic here, but it's currently not necessary since we're tied to @MainActor */
    static private(set) var seq = UInt64(0)
    static private func next() -> UInt64 {
        seq += 1
        return seq
    }
}
