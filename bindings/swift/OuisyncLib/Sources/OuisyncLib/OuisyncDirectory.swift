//
//  OuisyncDirectory.swift
//  
//
//  Created by Peter Jankuliak on 16/04/2024.
//

import Foundation
import System

public class OuisyncDirectory: CustomDebugStringConvertible {
    public let repository: OuisyncRepository
    public let path: FilePath

    public init(_ path: FilePath, _ repository: OuisyncRepository) {
        self.repository = repository
        self.path = path
    }

    public func name() -> String {
        return OuisyncDirectory.name(path)
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
            case 1: return OuisyncEntry.makeFile(path.appending(name), repository)
            case 2: return OuisyncEntry.makeDirectory(path.appending(name), repository)
            default:
                fatalError("Invalid EntryType returned from OuisyncLib \(typeNum)")
            }
        })
    }

    public func isRoot() -> Bool {
        return path.components.isEmpty
    }

    public func parent() -> OuisyncDirectory? {
        guard let parentPath = OuisyncDirectory.parent(path) else {
            return nil
        }
        return OuisyncDirectory(parentPath, repository)
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
