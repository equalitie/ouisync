import MessagePack


public extension Client {
    func shareToken(fromString value: String) async throws -> ShareToken {
        try await ShareToken(self, invoke("share_token_normalize", with: .string(value)))
    }
}


// this is a fucking secret too
public struct ShareToken: Secret {
    let client: Client
    public let value: MessagePackValue

    init(_ client: Client, _ value: MessagePackValue) {
        self.client = client
        self.value = value
    }

    /// The repository name suggested from this token.
    var suggestedName: String { get async throws {
        try await client.invoke("share_token_get_suggested_name", with: value).stringValue.orThrow
    } }

    var infoHash: String { get async throws {
        try await client.invoke("share_token_get_info_hash", with: value).stringValue.orThrow
    } }

    /// The access mode this token provides.
    var accessMode: AccessMode { get async throws {
        try await AccessMode(rawValue: client.invoke("share_token_get_access_mode",
                                                     with: value).uint8Value.orThrow).orThrow
    } }

    /// Check if the repository of this share token is mirrored on the cache server.
    func mirrorExists(at host: String) async throws -> Bool {
        try await client.invoke("share_token_mirror_exists",
                                with: ["share_token": value,
                                       "host": .string(host)]).boolValue.orThrow
    }
}
