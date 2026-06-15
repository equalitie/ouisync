import Foundation

extension Session {
    public static func create(configPath: String) async throws -> Session {
        let client = try await Client.connect(configPath: configPath)
        return Session(client)
    }

    public func close() async {
        await client.close()
    }
}
