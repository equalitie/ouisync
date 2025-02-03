import XCTest
import Ouisync


final class MultipleNodesTests: XCTestCase {
    var server1: Server!, client1: Client!, temp1: String!
    var server2: Server!, client2: Client!, temp2: String!
    var repo1, repo2: Repository!
    override func setUp() async throws {
        (server1, client1, temp1) = try await startServer(self, suffix: "-1")
        repo1 = try await client1.createRepository(at: "repo1")
        try await repo1.setSyncing(enabled: true)
        let token = try await repo1.share(for: .Write)
        try await client1.bindNetwork(to: ["quic/127.0.0.1:0"])

        (server2, client2, temp2) = try await startServer(self, suffix: "-2")
        repo2 = try await client2.createRepository(at: "repo2", importingFrom: token)
        try await repo2.setSyncing(enabled: true)
        try await client2.bindNetwork(to: ["quic/127.0.0.1:0"])
    }
    override func tearDown() async throws {
        var err: (any Error)!
        do { try await cleanupServer(server1, temp1) } catch { err = error }
        do { try await cleanupServer(server2, temp2) } catch { err = error }
        if err != nil { throw err }
    }

    func testNotificationOnSync() async throws {
        // expect one event for each block created (one for the root directory and one for the file)
        let stream = Task {
            var count = 0
            for try await _ in repo2.events {
                count += 1
                if count == 2 { break }
            }
        }
        try await client2.addUserProvidedPeers(from: client1.localListenerAddrs)
        _ = try await repo1.createFile(at: "file.txt")
        try await stream.value
    }

    func testNotificationOnPeersChange() async throws {
        let addr = try await client1.localListenerAddrs[0]
        let stream = Task {
            for try await _ in client2.networkEvents {
                for peer in try await client2.peers {
                    if peer.addr == addr,
                       case .UserProvided = peer.source,
                       case .Active = peer.state,
                       let _ = peer.runtimeId { return }
                }
            }
        }
        try await client2.addUserProvidedPeers(from: [addr])
        try await stream.value
    }

    func testNetworkStats() async throws {
        let addr = try await client1.localListenerAddrs[0]
        try await client2.addUserProvidedPeers(from: [addr])
        try await repo1.createFile(at: "file.txt").flush()

        // wait for the file to get synced
        for try await _ in repo2.events {
            do {
                _ = try await repo2.openFile(at: "file.txt")
                break
            } catch OuisyncError.NotFound, OuisyncError.StoreError {
                // FIXME: why does this also throw StoreError?
                continue
            }
        }

        let stats = try await client2.networkStats
        XCTAssertGreaterThan(stats.bytesTx, 0)
        XCTAssertGreaterThan(stats.bytesRx, 65536) // at least two blocks received
    }
}

