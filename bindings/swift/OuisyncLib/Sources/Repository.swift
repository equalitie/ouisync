import Foundation
import MessagePack


public extension Client {
     /** Creates a new repository (or imports an existing repository) with optional local encryption
      *
      * If a `token` is provided, the operation will be an `import`. Otherwise, a new, empty,
      * fully writable repository is created at `path`.
      *
      * The optional `readSecret` and `writeSecret` are intended to function as a second
      * authentication factor and are used to encrypt the repository's true access keys. Secrets
      * are ignored when the underlying `token` doesn't contain the corresponding key (e.g. for
      * "blind" or "read-only" tokens).
      *
      * Finally, due to our current key distribution mechanism, `writeSecret` becomes mandatory
      * when a `readSecret` is set. You may reuse the same secret for both values. */
    func createRepository(at path: String,
                          importingFrom token: ShareToken? = nil,
                          readSecret: Secret? = nil,
                          writeSecret: Secret? = nil) async throws {
        // FIXME: the backend does buggy things here, so we bail out; see also `unsafeSetSecrets`
        if readSecret != nil && writeSecret == nil { throw OuisyncError.Unsupported }
        try await invoke("repository_create", with: ["path": .string(path),
                                                     "read_secret": readSecret?.value ?? .nil,
                                                     "write_secret": writeSecret?.value ?? .nil,
                                                     "token": token?.value ?? .nil,
                                                     "sync_enabled": false,
                                                     "dht_enabled": false,
                                                     "pex_enabled": false])
    }

    /** Opens an existing repository from a `path`, optionally using a known `secret`.
     *
     * If the same repository is opened again, a new handle pointing to the same underlying
     * repository is returned. Closed automatically when all references go out of scope. */
    func openRepository(at path: String, using secret: Secret? = nil) async throws -> Repository {
        try await Repository(self, invoke("repository_open", with: ["path": .string(path),
                                                                    "secret": secret?.value ?? .nil,
                                                                    "sync_enabled": false]))
    }

    /// All currently open repositories.
    var repositories: [Repository] { get async throws {
        try await invoke("repository_list").arrayValue.orThrow.map { try Repository(self, $0) }
    } }
}


public class Repository {
    let client: Client
    let handle: MessagePackValue
    init(_ client: Client, _ handle: MessagePackValue) throws {
        _ = try handle.uintValue.orThrow
        self.client = client
        self.handle = handle
    }

    deinit {
        // we're going out of scope so we need to copy the state that the async closure will capture
        let client = client, handle = handle
        Task { try await client.invoke("repository_close", with: handle) }
    }
}


public extension Repository {
    /// Deletes this repository. It's an error to invoke any operation on it after it's been deleted.
    func delete() async throws {
        try await client.invoke("repository_delete", with: handle)
    }

    func move(to location: String) async throws {
        try await client.invoke("repository_move", with: ["repository": handle,
                                                          "to": .string(location)])
    }

    var path: String { get async throws {
        try await client.invoke("repository_get_path", with: handle).stringValue.orThrow
    } }

    /// Whether syncing with other replicas is enabled.
    var syncing: Bool { get async throws {
        try await client.invoke("repository_is_sync_enabled", with: handle).boolValue.orThrow
    } }

    /// Enables or disables syncing with other replicas.
    func setSyncing(enabled: Bool) async throws {
        try await client.invoke("repository_set_sync_enabled", with: ["repository": handle,
                                                                      "enabled": .bool(enabled)])
    }

    /** Resets access using `token` and reset any values encrypted with local secrets to random
     * values. Currently that is only the writer ID. */
    func resetAccess(using token: ShareToken) async throws {
        try await client.invoke("repository_reset_access", with: ["repository": handle,
                                                                  "token": token.value])
    }

    /** The current repository credentials.
     *
     * They can be used to restore repository access via `setCredentials()` after the repo has been
     * closed and re-opened, without needing the local secret (e.g. when moving the database). */
    var credentials: Data { get async throws {
        try await client.invoke("repository_credentials", with: handle).dataValue.orThrow
    } }
    func setCredentials(from credentials: Data) async throws {
        try await client.invoke("repository_set_credentials",
                                with: ["repository": handle, "credentials": .binary(credentials)])
    }

    var accessMode: AccessMode { get async throws {
        try await AccessMode(rawValue: client.invoke("repository_get_access_mode",
                                                     with: handle).uint8Value.orThrow).orThrow
    } }
    func setAccessMode(to mode: AccessMode, using secret: Secret? = nil) async throws {
        try await client.invoke("repository_set_access_mode",
                                with: ["repository": handle,
                                       "access_mode": .uint(UInt64(mode.rawValue)),
                                       "secret": secret?.value ?? .nil])
    }

    /// Returns the `EntryType` at `path`, or `nil` if there's nothing that location
    func entryType(at path: String) async throws -> EntryType? {
        let res = try await client.invoke("repository_entry_type", with: ["repository": handle,
                                                                          "path": .string(path)])
        if case .nil = res { return nil } // FIXME: this should just be an enum type
        return try EntryType(rawValue: res.uint8Value.orThrow).orThrow
    }

    /// Returns whether the entry (file or directory) at `path` exists.
    @available(*, deprecated, message: "use `entryType(at:)` instead")
    func entryExists(at path: String) async throws -> Bool { try await entryType(at: path) != nil }

    /// Move or rename the entry at `src` to `dst`
    func moveEntry(from src: String, to dst: String) async throws {
        try await client.invoke("repository_move_entry", with: ["repository": handle,
                                                                "src": .string(src),
                                                                "dst": .string(dst)])
    }

    /// This is a lot of overhead for a glorified event handler
    @MainActor var events: AsyncThrowingMapSequence<Client.Subscription, Void> {
        client.subscribe(to: "repository").map { _ in () }
    }

    var dht: Bool { get async throws {
        try await client.invoke("repository_is_dht_enabled", with: handle).boolValue.orThrow
    } }
    func setDht(enabled: Bool) async throws {
        try await client.invoke("repository_set_dht_enabled", with: ["repository": handle,
                                                                     "enabled": .bool(enabled)])
    }

    var pex: Bool { get async throws {
        try await client.invoke("repository_is_pex_enabled", with: handle).boolValue.orThrow
    } }
    func setPex(enabled: Bool) async throws {
        try await client.invoke("repository_set_pex_enabled", with: ["repository": handle,
                                                                     "enabled": .bool(enabled)])
    }

    /// Create a share token providing access to this repository with the given mode.
    func share(for mode: AccessMode, using secret: Secret? = nil) async throws -> ShareToken {
        try await ShareToken(client, client.invoke("repository_share",
                                                   with: ["repository": handle,
                                                          "secret": secret?.value ?? .nil,
                                                          "mode": .uint(UInt64(mode.rawValue))]))
    }

    var syncProgress: Progress { get async throws {
        try await Progress(client.invoke("repository_sync_progress", with: handle))
    } }

    var infoHash: String { get async throws {
        try await client.invoke("repository_get_info_hash", with: handle).stringValue.orThrow
    } }

    /// Create mirror of this repository on a cache server.
    func createMirror(to host: String) async throws {
        try await client.invoke("repository_create_mirror", with: ["repository": handle,
                                                                   "host": .string(host)])
    }

    /// Check if this repository is mirrored on a cache server.
    func mirrorExists(on host: String) async throws -> Bool {
        try await client.invoke("repository_mirror_exists",
                                with: ["repository": handle,
                                       "host": .string(host)]).boolValue.orThrow
    }

    /// Delete mirror of this repository from a cache server.
    func deleteMirror(from host: String) async throws {
        try await client.invoke("repository_delete_mirror", with: ["repository": handle,
                                                                   "host": .string(host)])
    }

    func metadata(for key: String) async throws -> String? {
        let res = try await client.invoke("repository_get_metadata", with: ["repository": handle,
                                                                            "key": .string(key)])
        if case .nil = res { return nil }
        return try res.stringValue.orThrow
    }

    /// Performs an (presumably atomic) CAS on `edits`, returning `true` if they were updated
    func updateMetadata(with edits: [String:(from: String, to: String)]) async throws -> Bool {
        try await client.invoke("repository_set_metadata",
                                with: ["repository": handle,
                                       "edits": .array(edits.map {["key": .string($0.key),
                                                                   "old": .string($0.value.from),
                                                                   "new": .string($0.value.to)]})]
        ).boolValue.orThrow
    }

    /// Mount the repository if supported by the platform.
    @available(*, deprecated, message: "Not supported on darwin")
    func mount() async throws {
        try await client.invoke("repository_mount", with: handle)
    }

    /// Unmount the repository.
    @available(*, deprecated, message: "Not supported on darwin")
    func unmount() async throws {
        try await client.invoke("repository_unmount", with: handle)
    }

    /// The mount point of this repository (or `nil` if not mounted).
    @available(*, deprecated, message: "Not supported on darwin")
    var mountPoint: String? { get async throws {
        let res = try await client.invoke("repository_get_mount_point", with: handle)
        if case .nil = res { return nil }
        return try res.stringValue.orThrow
    } }

    var networkStats: NetworkStats { get async throws {
        try await NetworkStats(client.invoke("repository_get_stats", with: handle))
    } }

    // FIXME: nominally called setAccess which is easily confused with setAccessMode
    /** Updates the secrets used to access this repository
     *
     * The default value maintains the existing key whereas explicitly passing `nil` removes it.
     *
     * `Known issue`: keeping an existing `read password` while removing the `write password` can
     * result in a `read-only` repository. If you break it, you get to keep all the pieces!
     */
    func unsafeSetSecrets(readSecret: Secret? = KEEP_EXISTING,
                          writeSecret: Secret? = KEEP_EXISTING) async throws {
        // FIXME: the implementation requires a distinction between "no key" and "remove key"...
        /** ...which is currently implemented in terms of a `well known` default value
         *
         * On a related matter, we are currently leaking an _important_ bit in the logs (the
         * existence or lack thereof of some secret) because we can't override `Optional.toString`.
         *
         * While the root cause is different, many languages have a similar
         * `nil is a singleton` problem which we can however fix via convention:
         *
         * If we agree that a secret of length `0` means `no password`, we can then use `nil` here
         * as a default argument for `keep existing secret` on both sides of the ffi */
        func encode(_ arg: Secret?) -> MessagePackValue {
            guard let arg else { return .string("disable") }
            return arg.value == KEEP_EXISTING.value ? .nil : ["enable": readSecret!.value]
        }
        try await client.invoke("repository_set_access", with: ["repository": handle,
                                                                "read": encode(readSecret),
                                                                "write": encode(writeSecret)])
    }
}

// feel free to swap this with your preferred subset of the output of `head /dev/random | base64`
@usableFromInline let KEEP_EXISTING = Password("i2jchEQApyAkAD79uPYvuO1jiTAumhAwwSOQx5GGuNu3NZPKc")
