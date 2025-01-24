import XCTest
import Ouisync


final class SessionTests: XCTestCase {
    var server: Server!, client: Client!, temp: String!
    override func setUp() async throws { (server, client, temp) = try await startServer(self) }
    override func tearDown() async throws { try await cleanupServer(server, temp) }

    func testThrowsWhenStartedTwice() async throws {
        try await XCTAssertThrows(try await startServer(self), OuisyncError.ServiceAlreadyRunning)
    }

    func testMultiClient() async throws {
        let other = try await server.connect()

        // this is a bit verbose to do concurrently because XCTAssert* uses non-async autoclosures
        async let future0 = client.currentProtocolVersion
        async let future1 = other.currentProtocolVersion
        let pair = try await (future0, future1)
        XCTAssertEqual(pair.0, pair.1)
    }

    func testUseAfterClose() async throws {
        try await server.stop()
        try await XCTAssertThrows(try await client.runtimeId, OuisyncError.ConnectionAborted)
    }
}
