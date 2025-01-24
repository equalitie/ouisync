import XCTest
import Ouisync


final class MoveEntryTests: XCTestCase {
    var server: Server!, client: Client!, temp: String!
    override func setUp() async throws { (server, client, temp) = try await startServer(self) }
    override func tearDown() async throws { try await cleanupServer(server, temp) }

    func testMoveEmptyFolder() async throws {
        // prep
        let repo = try await client.createRepository(at: "foo")
        try await repo.createDirectory(at: "/folder1");
        try await repo.createDirectory(at: "/folder1/folder2");

        // initial assertions
        var list = try await repo.listDirectory(at: "/")
        XCTAssertEqual(list.count, 1) // root only contains one entry (folder1)
        list = try await repo.listDirectory(at: "/folder1/folder2")
        XCTAssertEqual(list.count, 0) // folder2 is empty

        try await repo.moveEntry(from: "/folder1/folder2", to: "/folder2") // move folder2 to root

        // final assertions
        list = try await repo.listDirectory(at: "/folder1")
        XCTAssertEqual(list.count, 0) // folder1 is now empty
        list = try await repo.listDirectory(at: "/")
        XCTAssertEqual(list.count, 2) // root now contains folder1 AND folder2
    }

    func testMoveNonEmptyFolder() async throws {
        // prep
        let repo = try await client.createRepository(at: "bar")
        try await repo.createDirectory(at: "/folder1");
        try await repo.createDirectory(at: "/folder1/folder2");
        var file: File! = try await repo.createFile(at: "/folder1/folder2/file1.txt")
        let send = "hello world".data(using: .utf8)!
        try await file.write(send, toOffset: 0)
        try await file.flush()
        file = nil

        // initial assertions
        var list = try await repo.listDirectory(at: "/")
        XCTAssertEqual(list.count, 1) // root only contains one entry (folder1)
        list = try await repo.listDirectory(at: "/folder1/folder2")
        XCTAssertEqual(list.count, 1) // folder2 only contains one entry (file1)

        try await repo.moveEntry(from: "/folder1/folder2", to: "/folder2") // move folder2 to root

        // final assertions
        list = try await repo.listDirectory(at: "/folder1")
        XCTAssertEqual(list.count, 0) // folder1 is now empty
        list = try await repo.listDirectory(at: "/")
        XCTAssertEqual(list.count, 2) // root now contains folder1 AND folder2
        list = try await repo.listDirectory(at: "/folder2")
        XCTAssertEqual(list.count, 1) // folder2 still contains one entry (file1)
        file = try await repo.openFile(at: "/folder2/file1.txt")
        let recv = try await file.read(file.length, fromOffset: 0)
        XCTAssertEqual(send, recv) // file1 contains the same data it used to
    }
}
