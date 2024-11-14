/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

import Foundation

public enum OuisyncErrorCode: UInt16 {
    /// Store error
    case Store = 1
    /// Insuficient permission to perform the intended operation
    case PermissionDenied = 2
    /// Malformed data
    case MalformedData = 3
    /// Entry already exists
    case EntryExists = 4
    /// Entry doesn't exist
    case EntryNotFound = 5
    /// Multiple matching entries found
    case AmbiguousEntry = 6
    /// The intended operation requires the directory to be empty but it isn't
    case DirectoryNotEmpty = 7
    /// The indended operation is not supported
    case OperationNotSupported = 8
    /// Failed to read from or write into the config file
    case Config = 10
    /// Argument passed to a function is not valid
    case InvalidArgument = 11
    /// Request or response is malformed
    case MalformedMessage = 12
    /// Storage format version mismatch
    case StorageVersionMismatch = 13
    /// Connection lost
    case ConnectionLost = 14
    /// Invalid handle to a resource (e.g., Repository, File, ...)
    case InvalidHandle = 15
    /// Entry has been changed and no longer matches the expected value
    case EntryChanged = 16

    // These can't happen and apple devices
    // case VfsInvalidMountPoint = 2048
    // case VfsDriverInstall = 2049
    // case VfsBackend = 2050

    /// Unspecified error
    case Other = 65535
}

public class OuisyncError : Error, CustomDebugStringConvertible {
    public let code: OuisyncErrorCode
    public let message: String

    init(_ code: OuisyncErrorCode, _ message: String) {
        self.code = code
        self.message = message
    }

    public var debugDescription: String {
        let codeStr: String

        switch code {
        case .Store: codeStr = "Store error"
        case .PermissionDenied: codeStr = "Insuficient permission to perform the intended operation"
        case .MalformedData: codeStr = "Malformed data"
        case .EntryExists: codeStr = "Entry already exists"
        case .EntryNotFound: codeStr = "Entry doesn't exist"
        case .AmbiguousEntry: codeStr = "Multiple matching entries found"
        case .DirectoryNotEmpty: codeStr = "The intended operation requires the directory to be empty but it isn't"
        case .OperationNotSupported: codeStr = "The indended operation is not supported"
        case .Config: codeStr = "Failed to read from or write into the config file"
        case .InvalidArgument: codeStr = "Argument passed to a function is not valid"
        case .MalformedMessage: codeStr = "Request or response is malformed"
        case .StorageVersionMismatch: codeStr = "Storage format version mismatch"
        case .ConnectionLost: codeStr = "Connection lost"
        case .InvalidHandle: codeStr = "Invalid handle to a resource (e.g., Repository, File, ...)"
        case .EntryChanged: codeStr = "Entry has been changed and no longer matches the expected value"

        case .Other: codeStr = "Unspecified error"
        }

        return "OuisyncError(code:\(code), codeStr:\"\(codeStr)\", message:\"\(message)\")"
    }
}
