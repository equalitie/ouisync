import Foundation
import Network
import CryptoKit
import MessagePack

// ResponseResult wraps server replies: either a successful Response or an OuisyncError.
private enum ResponseResult {
    case success(Response)
    case failure(OuisyncError)
}

internal actor Client {
    private let connection: NWConnection
    private var nextId: UInt64 = 1
    private var pending: [UInt64: CheckedContinuation<Response, Error>] = [:]
    private var subscriptions: [UInt64: AsyncStream<Response>.Continuation] = [:]

    private init(_ connection: NWConnection) {
        self.connection = connection
    }

    // MARK: - Public API

    static func connect(configPath: String) async throws -> Client {
        // Read endpoint config
        let configFile = (configPath as NSString).appendingPathComponent("local_endpoint.conf")
        let configData = try Data(contentsOf: URL(fileURLWithPath: configFile))

        struct LocalEndpoint: Decodable {
            let port: Int
            let auth_key: String
        }
        let endpoint = try JSONDecoder().decode(LocalEndpoint.self, from: configData)

        guard endpoint.auth_key.count == 64,
              let authKeyData = Data(hexString: endpoint.auth_key)
        else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let authKey = SymmetricKey(data: authKeyData)

        // Connect
        let host = NWEndpoint.Host("127.0.0.1")
        let port = NWEndpoint.Port(integerLiteral: UInt16(endpoint.port))
        let conn = NWConnection(host: host, port: port, using: .tcp)

        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            conn.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    conn.stateUpdateHandler = nil
                    cont.resume()
                case .failed(let error):
                    conn.stateUpdateHandler = nil
                    cont.resume(throwing: error)
                case .cancelled:
                    conn.stateUpdateHandler = nil
                    cont.resume(throwing: CocoaError(.userCancelled))
                default:
                    break
                }
            }
            conn.start(queue: .global())
        }

        let client = Client(conn)
        try await client.runAuth(authKey: authKey)
        Task { await client.receiveLoop() }
        return client
    }

    func invoke(_ request: Request) async throws -> Response {
        let id = nextId
        nextId &+= 1

        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { cont in
                pending[id] = cont
                Task {
                    do {
                        try await sendFrame(id: id, request: request)
                    } catch {
                        if let c = pending.removeValue(forKey: id) {
                            c.resume(throwing: error)
                        }
                    }
                }
            }
        } onCancel: {
            Task {
                try? await sendCancel(id: id)
            }
        }
    }

    func subscribe(_ request: Request) async throws -> AsyncStream<Response> {
        let id = nextId
        nextId &+= 1

        var streamContinuation: AsyncStream<Response>.Continuation!
        let stream = AsyncStream<Response>(bufferingPolicy: .bufferingNewest(16)) { cont in
            streamContinuation = cont
        }
        subscriptions[id] = streamContinuation
        streamContinuation.onTermination = { [weak self] _ in
            Task { [weak self] in await self?.unsubscribe(id: id) }
        }

        do {
            return try await withTaskCancellationHandler {
                _ = try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Response, Error>) in
                    pending[id] = cont
                    Task {
                        do {
                            try await sendFrame(id: id, request: request)
                        } catch {
                            if let c = pending.removeValue(forKey: id) {
                                subscriptions.removeValue(forKey: id)?.finish()
                                c.resume(throwing: error)
                            }
                        }
                    }
                }
                return stream
            } onCancel: {
                Task { [weak self] in await self?.cancelSubscribe(id: id) }
            }
        } catch {
            subscriptions.removeValue(forKey: id)?.finish()
            throw error
        }
    }

    func close() {
        connection.cancel()
    }

    // MARK: - Auth

    private func runAuth(authKey: SymmetricKey) async throws {
        // 1. Send 256-byte client challenge
        var clientChallenge = [UInt8](repeating: 0, count: 256)
        for i in 0..<256 { clientChallenge[i] = UInt8.random(in: 0...255) }
        try await send(Data(clientChallenge))

        // 2. Receive 32-byte server proof + 256-byte server challenge
        let serverBytes = try await receiveExact(32 + 256)
        let serverProof = serverBytes.prefix(32)
        let serverChallenge = serverBytes.dropFirst(32)

        // 3. Verify server proof
        let expectedProof = HMAC<SHA256>.authenticationCode(
            for: Data(clientChallenge),
            using: authKey
        )
        guard Data(expectedProof) == Data(serverProof) else {
            connection.cancel()
            throw CocoaError(.fileReadNoPermission)
        }

        // 4. Send client proof
        let clientProof = HMAC<SHA256>.authenticationCode(
            for: Data(serverChallenge),
            using: authKey
        )
        try await send(Data(clientProof))
    }

    // MARK: - Frame encoding

    private func sendFrame(id: UInt64, request: Request) async throws {
        let payload = pack(request.encodeToMsgPack())
        let length = UInt32(8 + payload.count)

        var frame = Data(capacity: 4 + 8 + payload.count)
        frame.appendBigEndian(length)
        frame.appendBigEndian(id)
        frame.append(payload)
        try await send(frame)
    }

    private func sendCancel(id: UInt64) async throws {
        try await sendFrame(id: id, request: .cancel(MessageId(value: id)))
    }

    // MARK: - Receive loop

    private func receiveLoop() async {
        while true {
            do {
                // Read 4-byte length
                let lenData = try await receiveExact(4)
                let length = lenData.bigEndianUInt32

                // Read length bytes (contains 8-byte messageId + payload)
                guard length >= 8 else { continue }
                let body = try await receiveExact(Int(length))
                let messageId = body.prefix(8).bigEndianUInt64
                let payload = body.dropFirst(8)

                // Decode ResponseResult from msgpack
                guard let (value, _) = try? unpack(Data(payload)) else { continue }
                let result = decodeResponseResult(value)

                // Route to pending one-shot or active subscription stream
                if let cont = pending.removeValue(forKey: messageId) {
                    switch result {
                    case .success(let response):
                        cont.resume(returning: response)
                    case .failure(let error):
                        cont.resume(throwing: error)
                    }
                } else if let cont = subscriptions[messageId] {
                    switch result {
                    case .success(let response):
                        cont.yield(response)
                    case .failure:
                        cont.finish()
                        subscriptions.removeValue(forKey: messageId)
                    }
                }
            } catch {
                // Connection closed or error: fail all pending and close all streams
                let all = pending
                pending.removeAll()
                for cont in all.values {
                    cont.resume(throwing: error)
                }
                let allSubs = subscriptions
                subscriptions.removeAll()
                for cont in allSubs.values {
                    cont.finish()
                }
                return
            }
        }
    }

    // MARK: - Subscription helpers

    private func cancelSubscribe(id: UInt64) async {
        if let c = pending.removeValue(forKey: id) {
            subscriptions.removeValue(forKey: id)?.finish()
            c.resume(throwing: CancellationError())
        }
        try? await sendCancel(id: id)
    }

    private func unsubscribe(id: UInt64) async {
        guard subscriptions.removeValue(forKey: id) != nil else { return }
        try? await sendCancel(id: id)
    }

    // MARK: - NWConnection helpers

    private func send(_ data: Data) async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            connection.send(content: data, completion: .contentProcessed { error in
                if let e = error {
                    cont.resume(throwing: e)
                } else {
                    cont.resume()
                }
            })
        }
    }

    private func receiveExact(_ n: Int) async throws -> Data {
        var buf = Data()
        while buf.count < n {
            let chunk = try await receiveChunk(max: n - buf.count)
            buf.append(chunk)
        }
        return buf
    }

    private func receiveChunk(max: Int) async throws -> Data {
        try await withCheckedThrowingContinuation { cont in
            connection.receive(minimumIncompleteLength: 1, maximumLength: max) { data, _, isComplete, error in
                if let e = error {
                    cont.resume(throwing: e)
                } else if let d = data, !d.isEmpty {
                    cont.resume(returning: d)
                } else if isComplete {
                    cont.resume(throwing: CocoaError(.fileReadUnknown))
                } else {
                    cont.resume(throwing: CocoaError(.fileReadUnknown))
                }
            }
        }
    }
}

// MARK: - ResponseResult decoding

private func decodeResponseResult(_ v: MessagePackValue) -> ResponseResult {
    guard case .map(let m) = v, m.count == 1,
          let entry = m.first,
          case .string(let key) = entry.key
    else {
        return .failure(OuisyncError(.invalidData, "malformed response"))
    }

    switch key {
    case "Success":
        if let response = Response.decodeFromMsgPack(entry.value) {
            return .success(response)
        } else {
            return .failure(OuisyncError(.invalidData, "cannot decode response"))
        }
    case "Failure":
        guard case .array(let arr) = entry.value, arr.count >= 2,
              let codeRaw = { () -> UInt64? in
                  switch arr[0] { case .uint(let n): return n; case .int(let n): return UInt64(bitPattern: n); default: return nil }
              }(),
              let codeRepr = UInt16(exactly: codeRaw),
              let code = ErrorCode(rawValue: codeRepr),
              case .string(let message) = arr[1]
        else {
            return .failure(OuisyncError(.invalidData, "malformed failure response"))
        }
        var sources: [String] = []
        if arr.count > 2, case .array(let srcArr) = arr[2] {
            for s in srcArr {
                if case .string(let str) = s { sources.append(str) }
            }
        }
        return .failure(OuisyncError.dispatch(code, message, sources: sources))
    default:
        return .failure(OuisyncError(.invalidData, "unknown response key: \(key)"))
    }
}

// MARK: - Data extensions

private extension Data {
    init?(hexString: String) {
        let chars = Array(hexString)
        guard chars.count % 2 == 0 else { return nil }
        var bytes: [UInt8] = []
        for i in stride(from: 0, to: chars.count, by: 2) {
            guard let byte = UInt8(String(chars[i...i+1]), radix: 16) else { return nil }
            bytes.append(byte)
        }
        self = Data(bytes)
    }

    mutating func appendBigEndian(_ value: UInt32) {
        var v = value.bigEndian
        Swift.withUnsafeBytes(of: &v) { append(contentsOf: $0) }
    }

    mutating func appendBigEndian(_ value: UInt64) {
        var v = value.bigEndian
        Swift.withUnsafeBytes(of: &v) { append(contentsOf: $0) }
    }

    var bigEndianUInt32: UInt32 {
        var value: UInt32 = 0
        _ = Swift.withUnsafeMutableBytes(of: &value) { copyBytes(to: $0) }
        return value.bigEndian
    }

    var bigEndianUInt64: UInt64 {
        var value: UInt64 = 0
        _ = Swift.withUnsafeMutableBytes(of: &value) { copyBytes(to: $0) }
        return value.bigEndian
    }
}
