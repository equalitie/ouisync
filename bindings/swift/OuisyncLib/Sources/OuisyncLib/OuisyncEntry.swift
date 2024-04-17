//
//  OuisyncEntry.swift
//  
//
//  Created by Peter Jankuliak on 15/04/2024.
//

import Foundation
import System

public enum OuisyncEntry {
    case file(OuisyncFile)
    case directory(OuisyncDirectory)

    static func makeFile(_ path: FilePath, _ repo: OuisyncRepository) -> OuisyncEntry {
        return .file(OuisyncFile(path, repo))
    }

    static func makeDirectory(_ path: FilePath, _ repo: OuisyncRepository) -> OuisyncEntry {
        return .directory(OuisyncDirectory(path, repo))
    }

    public func name() -> String {
        switch self {
        case .file(let e): return e.name()
        case .directory(let e): return e.name()
        }
    }

    public func isDirectory() -> Bool {
        switch self {
        case .file: false
        case .directory: true
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
