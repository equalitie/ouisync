import Foundation
import MessagePack


public extension Repository {
    /** Lists all entries from an existing directory at `path`.
     *
     * Throws `OuisyncError` if `path` doesn't exist or is not a directory. */
    func listDirectory(at path: String) async throws -> [(name: String, type: EntryType)] {
        // FIXME: replace this with an AsyncStream to future-proof the API
        try await client.invoke("directory_read", with: ["repository": handle,
                                                         "path": .string(path)]).arrayValue.orThrow.map {
            guard let arr = $0.arrayValue, arr.count == 2 else { throw OuisyncError.InvalidData }
            return try (name: arr[0].stringValue.orThrow,
                        type: EntryType(rawValue: arr[1].uint8Value.orThrow).orThrow)
        }
    }

    /** Creates a new empty directory at `path`.
     *
     * Throws `OuisyncError` if `path` already exists of if the parent folder doesn't exist. */
    func createDirectory(at Path: String) async throws -> File {
        try await File(self, client.invoke("directory_create", with: ["repository": handle,
                                                                      "path": .string(path)]))
    }

    /** Remove a directory from `path`.
     *
     * If `recursive` is `false` (which is the default), the directory must be empty otherwise an
     * exception is thrown. Otherwise, the contents of the directory are also removed. */
    func remove(at path: String, recursive: Bool = false) async throws {
        try await client.invoke("directory_remove", with: ["repository": handle,
                                                           "path": .string(path),
                                                           "recursive": .bool(recursive)])
    }
}
