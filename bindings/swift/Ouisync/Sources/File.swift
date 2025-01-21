//
//  File.swift
//  OuisyncLib
//
//  Created by Radu Dan on 17.01.2025 and this generated comment was preserved because:
//
//  How many times throughout Xcode's history has `File.swift` ever been the intended name?

import Foundation
import MessagePack


public extension Repository {
    /** Opens an existing file from the current repository at `path`.
     *
     * Throws `OuisyncError` if `path` doesn't exist or is a directory. */
    func openFile(at path: String) async throws -> File {
        try await File(self, client.invoke("file_open", with: ["repository": handle,
                                                               "path": .string(path)]))
    }

    /** Creates a new file at `path`.
     *
     * Throws `OuisyncError` if `path` already exists of if the parent folder doesn't exist. */
    func createFile(at Path: String) async throws -> File {
        try await File(self, client.invoke("file_create", with: ["repository": handle,
                                                                 "path": .string(path)]))
    }

    /// Removes (deletes) the file at `path`.
    func removeFile(at path: String) async throws {
        try await client.invoke("file_remove", with: ["repository": handle,
                                                      "path": .string(path)])
    }
}


public class File {
    let repository: Repository
    let handle: MessagePackValue

    init(_ repo: Repository, _ handle: MessagePackValue) throws {
        _ = try handle.uint64Value.orThrow
        repository = repo
        self.handle = handle
    }

    /// Flush and close the handle once the file goes out of scope
    deinit {
        let client = repository.client, handle = handle
        Task { try await client.invoke("file_close", with: handle) }
    }
}


public extension File {
    /** Reads and returns at most `size` bytes from this file, starting at `offset`.
     *
     * Returns 0 bytes if `offset` is at or past the end of the file */
    func read(_ size: UInt64, fromOffset offset: UInt64) async throws -> Data {
        try await repository.client.invoke("file_read",
                                           with: ["file": handle,
                                                  "offset": .uint(offset),
                                                  "size": .uint(size)]).dataValue.orThrow
    }

    /// Writes `data` to this file, starting at `offset`
    func write(_ data: Data, toOffset offset: UInt64) async throws -> Data {
        try await repository.client.invoke("file_write",
                                           with: ["file": handle,
                                                  "offset": .uint(offset),
                                                  "data": .binary(data)]).dataValue.orThrow
    }

    /// Flushes any pending writes to persistent storage.
    func flush() async throws {
        try await repository.client.invoke("file_flush", with: handle)
    }

    /// Truncates the file to `len` bytes.
    func truncate(to len: UInt64 = 0) async throws {
        try await repository.client.invoke("file_truncate", with: ["file": handle,
                                                                   "len": .uint(len)])
    }

    var len: UInt64 { get async throws {
        try await repository.client.invoke("file_len", with: handle).uint64Value.orThrow
    } }

    var progress: UInt64 { get async throws {
        try await repository.client.invoke("file_progress", with: handle).uint64Value.orThrow
    } }
}
