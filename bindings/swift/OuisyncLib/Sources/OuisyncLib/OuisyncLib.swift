// The Swift Programming Language
// https://docs.swift.org/swift-book

import Foundation
import MessagePack

public typealias MessageId = UInt64
public typealias ErrorCode = Int64

public func listRepositories(_ messageId: MessageId) -> [UInt8] {
    return makeRequest(messageId, "list_repositories", MessagePackValue.nil)
}

func makeRequest(_ messageId: MessageId, _ function_name: String, _ arguments: MessagePackValue) -> [UInt8] {
    var message: [UInt8] = []
    message.append(contentsOf: withUnsafeBytes(of: messageId.bigEndian, Array.init))
    let m = [MessagePackValue.string(function_name): arguments]
    message.append(contentsOf: pack(MessagePackValue.map(m)))
    return message
}

public enum Response {
    case success(MessagePackValue)
    case error(ErrorCode, String)
    case notification(MessagePackValue)
}

public func parseResponse(_ data: [UInt8]) -> (MessageId, Response)? {
    let idByteCount = (MessageId.bitWidth / UInt8.bitWidth)

    if data.count < idByteCount {
        return nil
    }

    let bigEndianValue = data.withUnsafeBufferPointer {
             ($0.baseAddress!.withMemoryRebound(to: MessageId.self, capacity: 1) { $0 })
    }.pointee
    
    let id = MessageId(bigEndian: bigEndianValue)
    
    let unpacked = (try? unpack(Data(data[idByteCount...])))?.0
    NSLog("ZZZZZZZZZZZZZZZ \(unpacked)")
    if case let .map(m) = unpacked {
        if let success = m[.string("success")] {
            let optional_value = parseSuccess(success)
            if let value = optional_value {
                return (id, Response.success(value))
            }
        } else if let error = m[.string("failure")] {
            let optional_value = parseFailure(error)
            if let (ec, message) = optional_value {
                return (id, Response.error(ec, message))
            }
        } else if let notification = m[.string("notification")] {
            let optional_value = parseNotification(notification)
            if let value = optional_value {
                return (id, Response.notification(value))
            }
        }
    }

    return nil
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

func parseNotification(_ value: MessagePackValue) -> MessagePackValue? {
    if case let .map(m) = value {
        if m.count != 1 {
            return nil
        }
        return m.first!.value
    }
    return nil
}
