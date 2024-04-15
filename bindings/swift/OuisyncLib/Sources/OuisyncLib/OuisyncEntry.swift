//
//  OuisyncEntry.swift
//  
//
//  Created by Peter Jankuliak on 15/04/2024.
//

import Foundation
import System

public class OuisyncEntry: Equatable {
    public enum EntryType {
        case file
        case directory
    }

    public let path: FilePath
    public let type: EntryType

    public init(_ path: FilePath, _ type: EntryType) {
        self.path = path
        self.type = type
    }

    public func name() -> String {
        let l = path.lastComponent
        if l == nil && type == .directory {
            return "/"
        }
        return l!.string
    }

    public static func == (lhs: OuisyncEntry, rhs: OuisyncEntry) -> Bool {
        return lhs.type == rhs.type && lhs.path == rhs.path
    }

    func isDirectory() -> Bool {
        return type == .directory
    }
}
