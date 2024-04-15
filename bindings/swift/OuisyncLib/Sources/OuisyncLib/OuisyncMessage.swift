//
//  OuisyncMessage.swift
//  
//
//  Created by Peter Jankuliak on 15/04/2024.
//

import Foundation
import MessagePack
import System

//--------------------------------------------------------------------

public class MessageRequest {
    let functionName: String
    let functionArguments: MessagePackValue

    init(_ functionName: String, _ functionArguments: MessagePackValue) {
        self.functionName = functionName
        self.functionArguments = functionArguments
    }

    public static func listRepositories() -> MessageRequest {
        return MessageRequest("list_repositories", MessagePackValue.nil)
    }

    public static func subscribeToRepositoryListChange() -> MessageRequest {
        return MessageRequest("list_repositories_subscribe", MessagePackValue.nil)
    }

    public static func getRepositoryName(_ handle: RepositoryHandle) -> MessageRequest {
        return MessageRequest("repository_name", MessagePackValue(handle))
    }

    public static func listEntries(_ handle: RepositoryHandle, _ path: FilePath) -> MessageRequest {
        return MessageRequest("directory_open", MessagePackValue([
            MessagePackValue("repository"): MessagePackValue(handle),
            MessagePackValue("path"): MessagePackValue(path.description),
        ]))
    }
}

//--------------------------------------------------------------------

public class IncomingMessage {
    public let messageId: MessageId
    public let payload: IncomingPayload

    init(_ messageId: MessageId, _ payload: IncomingPayload) {
        self.messageId = messageId
        self.payload = payload
    }

    public static func deserialize(_ data: [UInt8]) -> IncomingMessage? {
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
                if let value = parseResponse(success) {
                    return IncomingMessage(id, IncomingPayload.response(value))
                }
            } else if let error = m[.string("failure")] {
                if let response = parseFailure(error) {
                    return IncomingMessage(id, IncomingPayload.error(response))
                }
            } else if let notification = m[.string("notification")] {
                if let value = parseNotification(notification) {
                    return IncomingMessage(id, IncomingPayload.notification(value))
                }
            }
        }

        return nil
    }
}

extension IncomingMessage: CustomStringConvertible {
    public var description: String {
        return "IncomingMessage(\(messageId), \(payload))"
    }
}

//--------------------------------------------------------------------

public enum IncomingPayload {
    case response(Response)
    case notification(OuisyncNotification)
    case error(ErrorResponse)
}

extension IncomingPayload: CustomStringConvertible {
    public var description: String {
        switch self {
        case .response(let response):
            return "response(\(response))"
        case .notification(let notification):
            return "notification(\(notification))"
         case .error(let error):
            return "error(\(error))"
        }
    }
}

//--------------------------------------------------------------------

public enum IncomingSuccessPayload {
    case response(Response)
    case notification(OuisyncNotification)
}

extension IncomingSuccessPayload: CustomStringConvertible {
    public var description: String {
        switch self {
        case .response(let value):
            return "response(\(value))"
        case .notification(let value):
            return "notificateion(\(value))"
        }
    }
}

//--------------------------------------------------------------------

public class Response {
    public let value: MessagePackValue
    init(_ value: MessagePackValue) {
        self.value = value
    }

    public func toData() -> Data? {
        return value.dataValue
    }

    public func toUInt64Array() -> [UInt64]? {
        class Fail: Error {}
        do {
            return try value.arrayValue?.map({ value in
                if let byte = value.uint64Value {
                    return byte
                }
                throw Fail()
            })
        } catch {
            return nil
        }
    }
}

extension Response: CustomStringConvertible {
    public var description: String {
        return "Response(\(value))"
    }
}

//--------------------------------------------------------------------

public class OuisyncNotification {
    let value: MessagePackValue
    init(_ value: MessagePackValue) {
        self.value = value
    }
}

extension OuisyncNotification: CustomStringConvertible {
    public var description: String {
        return "Notification(\(value))"
    }
}

//--------------------------------------------------------------------

public class ErrorResponse : Error {
    let errorCode: ErrorCode
    let message: String
    init(_ errorCode: ErrorCode, _ message: String) {
        self.errorCode = errorCode
        self.message = message
    }
}

extension ErrorResponse: CustomStringConvertible {
    public var description: String {
        return "ErrorResponse(\(errorCode), \(message))"
    }
}

//--------------------------------------------------------------------

func parseResponse(_ value: MessagePackValue) -> Response? {
    if case let .map(m) = value {
        if m.count != 1 {
            return nil
        }
        return Response(m.first!.value)
    }
    return nil
}

func parseFailure(_ value: MessagePackValue) -> ErrorResponse? {
    if case let .array(arr) = value {
        if arr.count != 2 {
            return nil
        }
        if case let .uint(code) = arr[0] {
            if case let .string(message) = arr[1] {
                return ErrorResponse(code, message)
            }
        }
    }
    return nil
}

func parseNotification(_ value: MessagePackValue) -> OuisyncNotification? {
    if case .string(_) = value {
        return OuisyncNotification(MessagePackValue.nil)
    }
    if case let .map(m) = value {
        if m.count != 1 {
            return nil
        }
        return OuisyncNotification(m.first!.value)
    }
    return nil
}
