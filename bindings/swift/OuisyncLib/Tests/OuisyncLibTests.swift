import XCTest
import OuisyncLib

final class OuisyncLibTests: XCTestCase {
    var tempDir: URL!
    var service: OuisyncService!
    var session: Session!

    override func setUp() async throws {
        tempDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("ouisync-\(UUID().uuidString)")
        let configDir = tempDir.appendingPathComponent("config").path
        let storeDir = tempDir.appendingPathComponent("store").path
        try FileManager.default.createDirectory(atPath: storeDir, withIntermediateDirectories: true)

        OuisyncService.initLog()
        service = try await OuisyncService.start(configDir: configDir)
        session = try await Session.create(configPath: configDir)
        try await session.setStoreDirs([storeDir])
    }

    override func tearDown() async throws {
        await session.close()
        try await service.stop()
        try? FileManager.default.removeItem(at: tempDir)
    }

    // MARK: - Repository tests

    func testListRepositoriesEmpty() async throws {
        let repos = try await session.listRepositories()
        XCTAssertTrue(repos.isEmpty)
    }

    func testCreateRepository() async throws {
        let repo = try await session.createRepository("repo")
        defer { Task { try? await repo.close() } }

        let repos = try await session.listRepositories()
        XCTAssertEqual(repos.count, 1)
    }

    func testDeleteRepository() async throws {
        let repo = try await session.createRepository("repo")
        try await repo.delete()

        let repos = try await session.listRepositories()
        XCTAssertTrue(repos.isEmpty)
    }

    func testMultipleRepositories() async throws {
        let a = try await session.createRepository("a")
        defer { Task { try? await a.close() } }
        let b = try await session.createRepository("b")
        defer { Task { try? await b.close() } }

        let repos = try await session.listRepositories()
        XCTAssertEqual(repos.count, 2)
    }

    // MARK: - File tests

    func testFileWriteAndRead() async throws {
        let content = "hello ouisync"
        let contentData = Data(content.utf8)

        let repo = try await session.createRepository("repo")
        defer { Task { try? await repo.close() } }

        let fileW = try await repo.createFile("test.txt")
        try await fileW.write(0, contentData)
        try await fileW.flush()
        try await fileW.close()

        let fileR = try await repo.openFile("test.txt")
        defer { Task { try? await fileR.close() } }
        let length = try await fileR.getLength()
        let data = try await fileR.read(0, length)

        XCTAssertEqual(data, contentData)
    }

    func testFileTruncate() async throws {
        let repo = try await session.createRepository("repo")
        defer { Task { try? await repo.close() } }

        let file = try await repo.createFile("test.txt")
        defer { Task { try? await file.close() } }

        try await file.write(0, Data("hello world".utf8))
        try await file.flush()
        XCTAssertEqual(try await file.getLength(), 11)

        try await file.truncate(5)
        XCTAssertEqual(try await file.getLength(), 5)

        let data = try await file.read(0, 5)
        XCTAssertEqual(String(data: data, encoding: .utf8), "hello")
    }

    func testOpenMissingFileThrows() async throws {
        let repo = try await session.createRepository("repo")
        defer { Task { try? await repo.close() } }

        do {
            _ = try await repo.openFile("missing.txt")
            XCTFail("Expected error opening missing file")
        } catch let e as OuisyncError where e.code == .notFound {
            // expected
        }
    }

    // MARK: - Session tests

    func testMultipleSessions() async throws {
        let configDir = tempDir.appendingPathComponent("config").path
        let other = try await Session.create(configPath: configDir)
        defer { Task { await other.close() } }

        let id1 = try await session.getRuntimeId()
        let id2 = try await other.getRuntimeId()
        XCTAssertEqual(id1.value, id2.value)
    }
}
