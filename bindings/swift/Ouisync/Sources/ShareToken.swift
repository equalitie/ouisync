import MessagePack


public extension Client {
    func shareToken(fromString value: String) async throws -> ShareToken {
        try await ShareToken(self, invoke("share_token_normalize", with: .string(value)))
    }
}


public struct ShareToken: Secret {
    // FIXME: should this retain the Client unless cast to string?
    let client: Client
    /// The (nominally secret) token value that should be stored or shared somewhere safe
    public let string: String
    public var value: MessagePackValue { .string(string) }

    init(_ client: Client, _ value: MessagePackValue) throws {
        self.client = client
        string = try value.stringValue.orThrow
    }
}


public extension ShareToken {
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
