// The Swift Programming Language
// https://docs.swift.org/swift-book

import Foundation
import MessagePack

public typealias MessageId = UInt64
public typealias ErrorCode = Int64

public func listRepositories(_ messageId: MessageId) -> [UInt8] {
    return makeRequest(messageId, "list_repositories", MessagePackValue.nil)
}

public func subscribeToRepositoryListChange(_ messageId: MessageId) -> [UInt8] {
    return makeRequest(messageId, "list_repositories_subscribe", MessagePackValue.nil)
}

public class Response {
    public let messageId: MessageId
    public let payload: ResponsePayload

    init(_ messageId: MessageId, _ payload: ResponsePayload) {
        self.messageId = messageId
        self.payload = payload
    }
}

extension Response: CustomStringConvertible {
    public var description: String {
        return "response(\(messageId), \(payload))"
    }
}

public enum ResponsePayload {
    case success(MessagePackValue)
    case error(ErrorCode, String)
    case notification(MessagePackValue)
}

extension ResponsePayload: CustomStringConvertible {
    public var description: String {
        switch self {
        case .success(let value):
            return "success(\(value))"
        case .error(let errorCode, let message):
            return "error(\(errorCode), \(message))"
        case .notification(let value):          return "notification(\(value))"
        }
    }
}

public func parseResponse(_ data: [UInt8]) -> Response? {
    let idByteCount = (MessageId.bitWidth / UInt8.bitWidth)

    if data.count < idByteCount {
        return nil
    }

    let bigEndianValue = data.withUnsafeBufferPointer {
        ($0.baseAddress!.withMemoryRebound(to: MessageId.self, capacity: 1) { $0 })
    }.pointee

    let id = MessageId(bigEndian: bigEndianValue)

    let unpacked = (try? unpack(Data(data[idByteCount...])))?.0

    if case let .map(m) = unpacked {
        if let success = m[.string("success")] {
            if let value = parseSuccess(success) {
                return Response(id, ResponsePayload.success(value))
            }
        } else if let error = m[.string("failure")] {
            if let (ec, message) = parseFailure(error) {
                return Response(id, ResponsePayload.error(ec, message))
            }
        } else if let notification = m[.string("notification")] {
            if let value = parseNotification(notification) {
                return Response(id, ResponsePayload.notification(value))
            }
        }
    }

    return nil
}

func makeRequest(_ messageId: MessageId, _ function_name: String, _ arguments: MessagePackValue) -> [UInt8] {
    var message: [UInt8] = []
    message.append(contentsOf: withUnsafeBytes(of: messageId.bigEndian, Array.init))
    let m = [MessagePackValue.string(function_name): arguments]
    message.append(contentsOf: pack(MessagePackValue.map(m)))
    return message
}

func parseSuccess(_ value: MessagePackValue) -> MessagePackValue? {
    if case let .map(m) = value {
        if m.count != 1 {
            return nil
        }
        return m.first!.value
    }
    return nil
}

func parseFailure(_ value: MessagePackValue) -> (ErrorCode, String)? {
    if case let .array(arr) = value {
        if arr.count != 2 {
            return nil
        }
        if case let .int(code) = arr[0] {
            if case let .string(message) = arr[1] {
                return (code, message)
            }
        }
    }
    return nil
}

/// Note that returning `nil` means parsing failed, but returning `MessagePackValue.nil` is a success
/// but the message had no payload.
func parseNotification(_ value: MessagePackValue) -> MessagePackValue? {
    if case .string(_) = value {
        return MessagePackValue.nil
    }
    if case let .map(m) = value {
        if m.count != 1 {
            return nil
        }
        return m.first!.value
    }
    return nil
}
