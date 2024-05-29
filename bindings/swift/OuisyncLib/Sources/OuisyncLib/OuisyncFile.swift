/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

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
