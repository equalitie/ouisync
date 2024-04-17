//
//  OuisyncFile.swift
//  
//
//  Created by Peter Jankuliak on 17/04/2024.
//

import Foundation
import System

public class OuisyncFile {
    public let repository: OuisyncRepository
    public let path: FilePath

    public init(_ path: FilePath, _ repository: OuisyncRepository) {
        self.repository = repository
        self.path = path
    }

    public func name() -> String {
        return OuisyncFile.name(path)
    }

    public static func name(_ path: FilePath) -> String {
        return path.lastComponent!.string
    }

    public func parent() -> OuisyncDirectory {
        return OuisyncDirectory(OuisyncFile.parent(path), repository)
    }

    public static func parent(_ path: FilePath) -> FilePath {
        var parentPath = path
        parentPath.components.removeLast()
        return parentPath
    }
}
