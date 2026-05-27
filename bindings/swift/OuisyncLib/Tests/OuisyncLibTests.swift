import Foundation
import Testing
import OuisyncLib

// Each @Test calls withFixture, which handles service + session lifecycle.
// This avoids relying on async deinit (not supported in Swift Testing).
// configDir is passed to the body so tests can open additional sessions.
private func withFixture(
    _ body: (_ session: Session, _ configDir: String) async throws -> Void
) async throws {
    let tempDir = FileManager.default.temporaryDirectory
        .appendingPathComponent("ouisync-\(UUID().uuidString)")
    let configDir = tempDir.appendingPathComponent("config").path
    let storeDir = tempDir.appendingPathComponent("store").path
    try FileManager.default.createDirectory(atPath: storeDir, withIntermediateDirectories: true)

    OuisyncService.initLog()
    let service = try await OuisyncService.start(configDir: configDir)
    let session = try await Session.create(configPath: configDir)
    try await session.setStoreDirs([storeDir])

    var bodyError: Error?
    do {
        try await body(session, configDir)
    } catch {
        bodyError = error
    }

    await session.close()
    try? await service.stop()
    try? FileManager.default.removeItem(at: tempDir)

    if let error = bodyError { throw error }
}

@Suite struct OuisyncLibTests {

    // MARK: - Repository

    @Test func listRepositoriesEmpty() async throws {
        try await withFixture { session, _ in
            let repos = try await session.listRepositories()
            #expect(repos.isEmpty)
        }
    }

    @Test func createRepository() async throws {
        try await withFixture { session, _ in
            let repo = try await session.createRepository("repo")
            defer { Task { try? await repo.close() } }

            let repos = try await session.listRepositories()
            #expect(repos.count == 1)
        }
    }

    @Test func deleteRepository() async throws {
        try await withFixture { session, _ in
            let repo = try await session.createRepository("repo")
            try await repo.delete()

            let repos = try await session.listRepositories()
            #expect(repos.isEmpty)
        }
    }

    @Test func multipleRepositories() async throws {
        try await withFixture { session, _ in
            let a = try await session.createRepository("a")
            defer { Task { try? await a.close() } }
            let b = try await session.createRepository("b")
            defer { Task { try? await b.close() } }

            let repos = try await session.listRepositories()
            #expect(repos.count == 2)
        }
    }

    // MARK: - File

    @Test func fileWriteAndRead() async throws {
        try await withFixture { session, _ in
            let content = Data("hello ouisync".utf8)
            let repo = try await session.createRepository("repo")
            defer { Task { try? await repo.close() } }

            let fileW = try await repo.createFile("test.txt")
            try await fileW.write(0, content)
            try await fileW.flush()
            try await fileW.close()

            let fileR = try await repo.openFile("test.txt")
            defer { Task { try? await fileR.close() } }
            let length = try await fileR.getLength()
            let data = try await fileR.read(0, length)
            #expect(data == content)
        }
    }

    @Test func fileTruncate() async throws {
        try await withFixture { session, _ in
            let repo = try await session.createRepository("repo")
            defer { Task { try? await repo.close() } }

            let file = try await repo.createFile("test.txt")
            defer { Task { try? await file.close() } }

            try await file.write(0, Data("hello world".utf8))
            try await file.flush()
            #expect(try await file.getLength() == 11)

            try await file.truncate(5)
            #expect(try await file.getLength() == 5)

            let slice = try await file.read(0, 5)
            #expect(String(data: slice, encoding: .utf8) == "hello")
        }
    }

    @Test func openMissingFileThrows() async throws {
        try await withFixture { session, _ in
            let repo = try await session.createRepository("repo")
            defer { Task { try? await repo.close() } }

            await #expect(throws: OuisyncError.NotFound.self) {
                _ = try await repo.openFile("missing.txt")
            }
        }
    }

    // MARK: - Session

    @Test func multipleSessionsSameRuntimeId() async throws {
        try await withFixture { session, configDir in
            let other = try await Session.create(configPath: configDir)
            defer { Task { await other.close() } }

            let id1 = try await session.getRuntimeId()
            let id2 = try await other.getRuntimeId()
            #expect(id1.value == id2.value)
        }
    }
}
