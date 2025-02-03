import XCTest
import Ouisync


final class RepositoryTests: XCTestCase {
    var server: Server!, client: Client!, temp: String!
    override func setUp() async throws { (server, client, temp) = try await startServer(self) }
    override func tearDown() async throws { try await cleanupServer(server, temp) }

    func testList() async throws {
        var repos = try await client.repositories
        XCTAssertEqual(repos.count, 0)
        let repo = try await client.createRepository(at: "foo")
        repos = try await client.repositories
        XCTAssertEqual(repos.count, 1)
        let id1 = try await repos[0].infoHash
        let id2 = try await repo.infoHash
        XCTAssertEqual(id1, id2)
    }

    func testFileIO() async throws {
        let repo = try await client.createRepository(at: "bar")
        let f1 = try await repo.createFile(at: "/test.txt")
        let send = "hello world".data(using: .utf8)!
        try await f1.write(send, toOffset: 0)
        try await f1.flush()
        let f2 = try await repo.openFile(at: "/test.txt")
        let recv = try await f2.read(f2.length, fromOffset: 0)
        XCTAssertEqual(send, recv)
    }

    func testDirectoryCreateAndRemove() async throws {
        let repo = try await client.createRepository(at: "baz")
        var stat = try await repo.entryType(at: "dir")
        XCTAssertNil(stat)
        var entries = try await repo.listDirectory(at: "/")
        XCTAssertEqual(entries.count, 0)

        try await repo.createDirectory(at: "dir")
        switch try await repo.entryType(at: "dir") {
        case .Directory: break
        default: XCTFail("Not a folder")
        }
        entries = try await repo.listDirectory(at: "/")
        XCTAssertEqual(entries.count, 1)
        XCTAssertEqual(entries[0].name, "dir")

        try await repo.removeDirectory(at: "dir")
        stat = try await repo.entryType(at: "dir")
        XCTAssertNil(stat)
        entries = try await repo.listDirectory(at: "/")
        XCTAssertEqual(entries.count, 0)
    }

    func testExternalRename() async throws {
        var repo = try await client.createRepository(at: "old")
        var files = try await repo.listDirectory(at: "/")
        XCTAssertEqual(files.count, 0)
        var file  = try await repo.createFile(at: "file.txt")
        let send = "hello world".data(using: .utf8)!
        try await file.write(send, toOffset: 0)
        try await file.flush()
        files = try await repo.listDirectory(at: "/")
        XCTAssertEqual(files.count, 1)
        try await server.stop()

        // manually move the repo database
        let src = temp + "/store/old.ouisyncdb"
        let dst = temp + "/store/new.ouisyncdb"
        let fm = FileManager.default
        try fm.moveItem(atPath: src, toPath: dst)
        // optionally also move the wal and shmem logs if enabled
        for tail in ["-wal", "-shm"] { try? fm.moveItem(atPath: src + tail, toPath: dst + tail) }

        // restart server and ensure repo is opened from the new location
        (server, client, _) = try await startServer(self)
        let repos = try await client.repositories
        XCTAssertEqual(repos.count, 1)
        repo = repos[0]
        let path = try await repo.path
        XCTAssert(path.hasSuffix("new.ouisyncdb"))

        // make sure the file was moved as well
        files = try await repo.listDirectory(at: "/")
        XCTAssertEqual(files.count, 1)
        file = try await repo.openFile(at: "file.txt")
        let recv = try await file.read(file.length, fromOffset: 0)
        XCTAssertEqual(send, recv)
    }

    func testDropsCredentialsOnRestart() async throws {
        // create a new password-protected repository
        let pass = Password("foo")
        var repo = try await client.createRepository(at: "bip", readSecret: pass, writeSecret: pass)
        var mode = try await repo.accessMode
        XCTAssertEqual(mode, .Write)

        // test that credentials are not persisted across server restarts
        try await server.stop()
        (server, client, _) = try await startServer(self)
        repo = try await client.repositories[0]
        mode = try await repo.accessMode
        XCTAssertEqual(mode, .Blind)

        // check that the credentials are however stored to disk
        try await repo.setAccessMode(to: .Write, using: pass)
        mode = try await repo.accessMode
        XCTAssertEqual(mode, .Write)
    }

    func testMetadata() async throws {
        let repo = try await client.createRepository(at: "bop")
        var val = try await repo.metadata["test.foo"]
        XCTAssertNil(val)
        val = try await repo.metadata["test.bar"]
        XCTAssertNil(val)
        var changed = try await repo.metadata.update(["test.foo": (from: nil, to: "foo 1"),
                                                      "test.bar": (from: nil, to: "bar 1")])
        XCTAssert(changed)
        val = try await repo.metadata["test.foo"]
        XCTAssertEqual(val, "foo 1")
        val = try await repo.metadata["test.bar"]
        XCTAssertEqual(val, "bar 1")

        // `from` and `to` are optional but recommended for readability (as seen below):
        changed = try await repo.metadata.update(["test.foo": ("foo 1", to: "foo 2"),
                                                  "test.bar": (from: "bar 1", nil)])
        XCTAssert(changed)
        val = try await repo.metadata["test.foo"]
        XCTAssertEqual(val, "foo 2")
        val = try await repo.metadata["test.bar"]
        XCTAssertNil(val)

        // old value mismatch
        changed = try await repo.metadata.update(["test.foo": (from: "foo 1", to: "foo 3")])
        XCTAssert(!changed)
        val = try await repo.metadata["test.foo"]
        XCTAssertEqual(val, "foo 2")

        // multi-value updates are rolled back atomically
        changed = try await repo.metadata.update(["test.foo": (from: "foo 1", to: "foo 4"),
                                                  "test.bar": (from: nil, to: "bar 4")])
        XCTAssert(!changed)
        val = try await repo.metadata["test.bar"]
        XCTAssertNil(val)
    }

    func testShareTokens() async throws {
        let repo = try await client.createRepository(at: "pop")
        var validToken: String!

        // test sharing returns a corresponding token
        for src in [AccessMode.Blind, AccessMode.Read, AccessMode.Write] {
            let token = try await repo.share(for: src)
            let dst = try await token.accessMode
            validToken = token.string // keep a ref to a valid token
            XCTAssertEqual(src, dst)
        }

        // ensure that valid tokens are parsed correctly
        let tok = try await client.shareToken(fromString: validToken)
        let mode = try await tok.accessMode
        XCTAssertEqual(mode, .Write)
        let name = try await tok.suggestedName
        XCTAssertEqual(name, "pop")

        // ensure that the returned infohash appears valid
        let hash = try await tok.infoHash
        XCTAssertNotNil(hash.wholeMatch(of: try! Regex("[0-9a-fA-F]{40}")))

        // ensure that invalid tokens throw an error
        // FIXME: should throw .InvalidInput instead
        try await XCTAssertThrows(try await client.shareToken(fromString: "broken!@#%"),
                                  OuisyncError.InvalidData)
    }

    func testUserProvidedPeers() async throws {
        var peers = try await client.userProvidedPeers
        XCTAssertEqual(peers.count, 0)

        try await client.addUserProvidedPeers(from: ["quic/127.0.0.1:12345",
                                                     "quic/127.0.0.2:54321"])
        peers = try await client.userProvidedPeers
        XCTAssertEqual(peers.count, 2)

        try await client.removeUserProvidedPeers(from: ["quic/127.0.0.2:54321",
                                                        "quic/127.0.0.2:13337"])
        peers = try await client.userProvidedPeers
        XCTAssertEqual(peers.count, 1)
        XCTAssertEqual(peers[0], "quic/127.0.0.1:12345")

        try await client.removeUserProvidedPeers(from: ["quic/127.0.0.1:12345"])
        peers = try await client.userProvidedPeers
        XCTAssertEqual(peers.count, 0)
    }

    func testPadCoverage() async throws {
        // these are ripped from `ouisync_test.dart` and don't really test much other than ensuring
        // that their underlying remote procedure calls can work under the right circumstances
        let repo = try await client.createRepository(at: "mop")

        // sync progress
        let progress = try await repo.syncProgress
        XCTAssertEqual(progress.value, 0)
        XCTAssertEqual(progress.total, 0)

        // state monitor
        let root = await client.root
        XCTAssertEqual(root.children.count, 0)
        let exists = try await root.load()
        XCTAssert(exists)
        XCTAssertEqual(root.children.count, 3)
    }

    func testNetwork() async throws {
        try XCTSkipUnless(envFlag("INCLUDE_SLOW"))

        try await client.bindNetwork(to: ["quic/0.0.0.0:0", "quic/[::]:0"])
        async let v4 = client.externalAddressV4
        async let v6 = client.externalAddressV4
        async let pnat = client.natBehavior
        let (ipv4, ipv6, nat) = try await (v4, v6, pnat)
        XCTAssertFalse(ipv4.isEmpty)
        XCTAssertFalse(ipv6.isEmpty)
        XCTAssert(["endpoint independent",
                   "address dependent",
                   "address and port dependent"].contains(nat))
    }
}
