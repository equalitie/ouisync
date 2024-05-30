/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

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

    public static func subscribeToRepositoryChange(_ handle: RepositoryHandle) -> MessageRequest {
        return MessageRequest("repository_subscribe", MessagePackValue(handle))
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

    public static func directoryExists(_ handle: RepositoryHandle, _ path: FilePath) -> MessageRequest {
        return MessageRequest("directory_exists", MessagePackValue([
            MessagePackValue("repository"): MessagePackValue(handle),
            MessagePackValue("path"): MessagePackValue(path.description),
        ]))
    }

    public static func fileOpen(_ repoHandle: RepositoryHandle, _ path: FilePath) -> MessageRequest {
        return MessageRequest("file_open", MessagePackValue([
            MessagePackValue("repository"): MessagePackValue(repoHandle),
            MessagePackValue("path"): MessagePackValue(path.description),
        ]))
    }

    public static func fileExists(_ handle: RepositoryHandle, _ path: FilePath) -> MessageRequest {
        return MessageRequest("file_exists", MessagePackValue([
            MessagePackValue("repository"): MessagePackValue(handle),
            MessagePackValue("path"): MessagePackValue(path.description),
        ]))
    }

    public static func fileClose(_ fileHandle: FileHandle) -> MessageRequest {
        return MessageRequest("file_close", MessagePackValue([
            MessagePackValue("file"): MessagePackValue(fileHandle),
        ]))
    }

    public static func fileRead(_ fileHandle: FileHandle, _ offset: UInt64, _ len: UInt64) -> MessageRequest {
        return MessageRequest("file_read", MessagePackValue([
            MessagePackValue("file"): MessagePackValue(fileHandle),
            MessagePackValue("offset"): MessagePackValue(offset),
            MessagePackValue("len"): MessagePackValue(len),
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
    case error(OuisyncError)
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

    // Note about unwraps in these methods. It is expected that the
    // caller knows what type the response is. If the expected and
    // the actual types differ, then it is likely that there is a
    // mismatch between the front end and the backend in the FFI API.

    init(_ value: MessagePackValue) {
        self.value = value
    }

    public func toData() -> Data {
        return value.dataValue!
    }

    public func toUInt64Array() -> [UInt64] {
        return value.arrayValue!.map({ $0.uint64Value! })
    }

    public func toUInt64() -> UInt64 {
        return value.uint64Value!
    }

    public func toBool() -> Bool {
        return value.boolValue!
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

func parseResponse(_ value: MessagePackValue) -> Response? {
    if case let .map(m) = value {
        if m.count != 1 {
            return nil
        }
        return Response(m.first!.value)
    }
    return nil
}

func parseFailure(_ value: MessagePackValue) -> OuisyncError? {
    if case let .array(arr) = value {
        if arr.count != 2 {
            return nil
        }
        if case let .uint(code) = arr[0] {
            if case let .string(message) = arr[1] {
                guard let codeU16 = UInt16(exactly: code) else {
                    fatalError("Error code from backend is out of range")
                }
                guard let codeEnum = OuisyncErrorCode(rawValue: codeU16) else {
                    fatalError("Invalid error code from backend")
                }
                return OuisyncError(codeEnum, message)
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
