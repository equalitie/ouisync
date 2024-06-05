/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

import Foundation
import System

public enum OuisyncEntryType {
    case file
    case directory
}

public enum OuisyncEntry {
    case file(OuisyncFileEntry)
    case directory(OuisyncDirectoryEntry)

    public func name() -> String {
        switch self {
        case .file(let e): return e.name()
        case .directory(let e): return e.name()
        }
    }

    public func type() -> OuisyncEntryType {
        switch self {
        case .file: return .file
        case .directory: return .directory
        }
    }

    public func parent() -> OuisyncEntry? {
        switch self {
        case .file(let file): return .directory(file.parent())
        case .directory(let directory):
            guard let parent = directory.parent() else {
                return nil
            }
            return .directory(parent)
        }
    }
}

public class OuisyncFileEntry {
    public let path: FilePath
    public let repository: OuisyncRepository

    public init(_ path: FilePath, _ repository: OuisyncRepository) {
        self.path = path
        self.repository = repository
    }

    public func parent() -> OuisyncDirectoryEntry {
        return OuisyncDirectoryEntry(Self.parent(path), repository)
    }

    public func name() -> String {
        return Self.name(path)
    }

    public static func name(_ path: FilePath) -> String {
        return path.lastComponent!.string
    }

    public func exists() async throws -> Bool {
        return try await repository.session.sendRequest(.fileExists(repository.handle, path)).toBool()
    }

    public func delete() async throws {
        try await repository.deleteFile(path)
    }

    public static func parent(_ path: FilePath) -> FilePath {
        var parentPath = path
        parentPath.components.removeLast()
        return parentPath
    }

    public func open() async throws -> OuisyncFile {
        let fileHandle = try await repository.session.sendRequest(.fileOpen(repository.handle, path)).toUInt64()
        return OuisyncFile(fileHandle, repository)
    }
}

public class OuisyncDirectoryEntry: CustomDebugStringConvertible {
    public let repository: OuisyncRepository
    public let path: FilePath

    public init(_ path: FilePath, _ repository: OuisyncRepository) {
        self.repository = repository
        self.path = path
    }

    public func name() -> String {
        return OuisyncDirectoryEntry.name(path)
    }

    public static func name(_ path: FilePath) -> String {
        if let c = path.lastComponent {
            return c.string
        }
        return "/"
    }

    public func listEntries() async throws -> [OuisyncEntry] {
        let response = try await repository.session.sendRequest(MessageRequest.listEntries(repository.handle, path))
        let entries = response.value.arrayValue!
        return entries.map({entry in
            let name: String = entry[0]!.stringValue!
            let typeNum = entry[1]!.uint8Value!

            switch typeNum {
            case 1: return .file(OuisyncFileEntry(path.appending(name), repository))
            case 2: return .directory(OuisyncDirectoryEntry(path.appending(name), repository))
            default:
                fatalError("Invalid EntryType returned from OuisyncLib \(typeNum)")
            }
        })
    }

    public func isRoot() -> Bool {
        return path.components.isEmpty
    }

    public func parent() -> OuisyncDirectoryEntry? {
        guard let parentPath = OuisyncDirectoryEntry.parent(path) else {
            return nil
        }
        return OuisyncDirectoryEntry(parentPath, repository)
    }

    public func exists() async throws -> Bool {
        let response = try await repository.session.sendRequest(MessageRequest.directoryExists(repository.handle, path))
        return response.value.boolValue!
    }

    public func delete(recursive: Bool) async throws {
        try await repository.deleteDirectory(path, recursive: recursive)
    }

    public static func parent(_ path: FilePath) -> FilePath? {
        if path.components.isEmpty {
            return nil
        } else {
            var parentPath = path
            parentPath.components.removeLast()
            return parentPath
        }
    }

    public var debugDescription: String {
        return "OuisyncDirectory(\(path), \(repository))"
    }
}
