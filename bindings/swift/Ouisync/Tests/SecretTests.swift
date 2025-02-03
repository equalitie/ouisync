import XCTest
import Ouisync


fileprivate extension Repository {
    func dropCredentials() async throws {
        try await setAccessMode(to: .Blind)
        let mode = try await accessMode
        XCTAssertEqual(mode, .Blind)
    }
}


final class SecretTests: XCTestCase {
    var server: Server!, client: Client!, temp: String!
    override func setUp() async throws { (server, client, temp) = try await startServer(self) }
    override func tearDown() async throws { try await cleanupServer(server, temp) }

    func testOpeningRepoUsingKeys() async throws {
        let readSecret = try SaltedSecretKey.random
        let writeSecret = try SaltedSecretKey.random
        let repo = try await client.createRepository(at: "repo1",
                                                     readSecret: readSecret,
                                                     writeSecret: writeSecret)
        // opened for write by default
        var mode = try await repo.accessMode
        XCTAssertEqual(mode, .Write)
        try await repo.dropCredentials()

        // reopen for reading using a read key
        try await repo.setAccessMode(to: .Read, using: readSecret.key)
        mode = try await repo.accessMode
        XCTAssertEqual(mode, .Read)
        try await repo.dropCredentials()

        // reopen for reading using a write key
        try await repo.setAccessMode(to: .Read, using: writeSecret.key)
        mode = try await repo.accessMode
        XCTAssertEqual(mode, .Read)
        try await repo.dropCredentials()

        // attempt reopen for writing using a read key (fails but defaults to read)
        try await repo.setAccessMode(to: .Write, using: readSecret.key)
        mode = try await repo.accessMode
        XCTAssertEqual(mode, .Read)
        try await repo.dropCredentials()

        // attempt reopen for writing using a write key
        try await repo.setAccessMode(to: .Write, using: writeSecret.key)
        mode = try await repo.accessMode
        XCTAssertEqual(mode, .Write)
        try await repo.dropCredentials()
    }

    func testCreateUsingKeyOpenWithPassword() async throws {
        let readPassword = Password("foo")
        let writePassword = Password("bar")

        let readSalt = try await client.generateSalt()
        let writeSalt = try Salt.random

        let readKey = try await client.deriveSecretKey(from: readPassword, with: readSalt)
        let writeKey = try await client.deriveSecretKey(from: writePassword, with: writeSalt)

        let repo = try await client.createRepository(at: "repo2", readSecret: readKey,
                                                                  writeSecret: writeKey)

        // opened for write by default
        var mode = try await repo.accessMode
        XCTAssertEqual(mode, .Write)
        try await repo.dropCredentials()

        // reopen for reading using the read password
        try await repo.setAccessMode(to: .Read, using: readPassword)
        mode = try await repo.accessMode
        XCTAssertEqual(mode, .Read)
        try await repo.dropCredentials()

        // reopen for reading using the write password
        try await repo.setAccessMode(to: .Read, using: writePassword)
        mode = try await repo.accessMode
        XCTAssertEqual(mode, .Read)
        try await repo.dropCredentials()

        // attempt reopen for writing using the read password (fails but defaults to read)
        try await repo.setAccessMode(to: .Write, using: readPassword)
        mode = try await repo.accessMode
        XCTAssertEqual(mode, .Read)
        try await repo.dropCredentials()

        // attempt reopen for writing using the write password
        try await repo.setAccessMode(to: .Write, using: writePassword)
        mode = try await repo.accessMode
        XCTAssertEqual(mode, .Write)
        try await repo.dropCredentials()
    }
}
