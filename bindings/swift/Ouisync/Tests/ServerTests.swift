import XCTest
@testable import Ouisync


final class SessionTests: XCTestCase {
    var server: Server!

    override func setUp() async throws {
        server = try await startServer(self)
    }

    override func tearDown() async throws {
        try await server.destroy()
    }

    func testThrowsWhenStartedTwice() async throws {
        do {
            let server2 = try await startServer(self)
            XCTFail("Did not throw")
        } catch OuisyncError.ServiceAlreadyRunning {
            // expected outcome, other errors should propagate and fail
        }
    }

    func testMultiSession() async throws {
        let client0 = try await server.connect()
        let client1 = try await server.connect()

        // this is a bit verbose to do concurrently because XCTAssertEqual uses non-async autoclosures
        let pair = try await (client0.currentProtocolVersion, client1.currentProtocolVersion)
        XCTAssertEqual(pair.0, pair.1)
    }
}
