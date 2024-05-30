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
        return try await repository.session.sendRequest(.fileRead(handle, offset, length)).toData()
    }

    public func close() async throws {
        let _ = try await repository.session.sendRequest(.fileClose(handle))
    }
}
