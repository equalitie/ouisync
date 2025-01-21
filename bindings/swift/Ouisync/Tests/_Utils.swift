import XCTest
@testable import Ouisync


func startServer(_ test: XCTestCase) async throws -> Server {
    ouisyncLogHandler = { level, message in print(message) }
    let path = test.name.replacingOccurrences(of: "-[", with: "")
                        .replacingOccurrences(of: "]", with: "")
                        .replacingOccurrences(of: " ", with: "_")
    let config = URL.temporaryDirectory.appending(path: path, directoryHint: .isDirectory)
    try FileManager.default.createDirectory(at: config, withIntermediateDirectories: true)
    return try await Server(configDir: config.absoluteString, debugLabel: "ouisync")
}

extension Server {
    func destroy() async throws {
        print("deleting \(configDir.absoluteString)")
        try await stop()
        try FileManager.default.removeItem(at: configDir)
    }
}
