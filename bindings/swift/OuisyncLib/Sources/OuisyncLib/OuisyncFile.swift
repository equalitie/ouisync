/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

import Foundation
import System

public class OuisyncFile {
    public let repository: OuisyncRepository
    let handle: FileHandle

    init(_ handle: FileHandle, _ repository: OuisyncRepository) {
        self.repository = repository
        self.handle = handle
    }

    public func read(_ offset: UInt64, _ length: UInt64) async throws -> Data {
        try await repository.session.sendRequest(.fileRead(handle, offset, length)).toData()
    }

    public func write(_ offset: UInt64, _ data: Data) async throws {
        let _ = try await repository.session.sendRequest(.fileWrite(handle, offset, data))
    }

    public func size() async throws -> UInt64 {
        try await repository.session.sendRequest(.fileLen(handle)).toUInt64()
    }

    public func close() async throws {
        let _ = try await repository.session.sendRequest(.fileClose(handle))
    }
}
