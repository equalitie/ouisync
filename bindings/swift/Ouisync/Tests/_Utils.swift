import Ouisync
import XCTest


func startServer(_ test: XCTestCase, suffix: String = "") async throws -> (Server, Client, String) {
    if !envFlag("ENABLE_LOGGING") { ouisyncLogHandler = { level, message in print(message) } }
    let path = test.name.replacingOccurrences(of: "-[", with: "")
                        .replacingOccurrences(of: "]", with: "")
                        .replacingOccurrences(of: " ", with: "_") + suffix
    let temp = URL.temporaryDirectory.path(percentEncoded: true).appending(path)
    try FileManager.default.createDirectory(atPath: temp, withIntermediateDirectories: true)
    let server = try await Server(configDir: temp.appending("/config"), debugLabel: "ouisync")
    let client = try await server.connect()
    try await client.setStoreDir(to: temp.appending("/store"))
    return (server, client, temp)
}

func cleanupServer(_ server: Server!, _ path: String) async throws {
    defer { try? FileManager.default.removeItem(atPath: path) }
    try await server?.stop()
}


// TODO: support non-equatable errors via a filter function
func XCTAssertThrows<T, E>(_ expression: @autoclosure () async throws -> T,
                           _ filter: E,
                           file: StaticString = #filePath,
                           line: UInt = #line) async throws where E: Error, E: Equatable {
    do {
        _ = try await expression()
        XCTFail("Did not throw", file: file, line: line)
    } catch let error as E where error == filter {}
}


// true iff env[key] is set to a non-empty string that is different from "0"
func envFlag(_ key: String) -> Bool {
    guard let val = ProcessInfo.processInfo.environment[key], val.count > 0 else { return false }
    return Int(val) != 0
}
