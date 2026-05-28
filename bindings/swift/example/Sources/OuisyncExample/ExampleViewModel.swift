import Foundation
import OuisyncLib

private let configDir: String = {
    let appSupport = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
    return appSupport.appendingPathComponent("OuisyncExample/config").path
}()

private let storeDir: String = {
    let appSupport = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
    return appSupport.appendingPathComponent("OuisyncExample/store").path
}()

@MainActor
class ExampleViewModel: ObservableObject {
    private var service: OuisyncService?
    private var session: Session?

    @Published var sessionError: String?
    @Published var repositories: [String: Repository] = [:]

    init() {
        Task { await start() }
    }

    // MARK: - Lifecycle

    private func start() async {
        try? FileManager.default.createDirectory(atPath: storeDir, withIntermediateDirectories: true)

        OuisyncService.initLog()

        do {
            service = try await OuisyncService.start(configDir: configDir)
        } catch let error as OuisyncError where error.code == .serviceAlreadyRunning {
            // another instance started the service, connect anyway
        } catch {
            sessionError = "Service failed to start: \(error)"
            return
        }

        do {
            session = try await Session.create(configPath: configDir)
            try await session?.setStoreDirs([storeDir])
        } catch {
            sessionError = "Session failed to connect: \(error)"
            return
        }

        // Bind to all interfaces on random ports, QUIC only, IPv4 + IPv6.
        try? await session?.bindNetwork(["quic/0.0.0.0:0", "quic/[::]:0"])
        // UPnP improves reachability behind NAT.
        try? await session?.setPortForwardingEnabled(true)
        // Automatically discover peers on the LAN.
        try? await session?.setLocalDiscoveryEnabled(true)

        await loadRepositories()
    }

    func shutdown() async {
        let repos = repositories.values
        repositories = [:]
        for repo in repos {
            try? await repo.close()
        }
        await session?.close()
        session = nil
        try? await service?.stop()
        service = nil
    }

    // MARK: - Repository management

    func loadRepositories() async {
        guard let session else { return }
        do {
            repositories = try await session.listRepositories()
        } catch {
            sessionError = "Failed to list repositories: \(error)"
        }
    }

    func createRepository(name: String, token: String) async throws {
        guard let session else { return }

        var shareToken: ShareToken? = nil
        if !token.isEmpty {
            shareToken = try await session.validateShareToken(token)
        }

        // syncEnabled / dhtEnabled / pexEnabled all true so the new repo starts syncing immediately.
        let repo = try await session.createRepository(name, nil, nil, shareToken, true, true, true)
        repositories[name] = repo
    }

    func deleteRepository(name: String) async throws {
        guard let repo = repositories[name] else { return }
        repositories.removeValue(forKey: name)
        try await repo.delete()
    }

    func shareRepository(name: String) async -> String? {
        guard let repo = repositories[name] else { return nil }
        return try? await repo.share(.write).value
    }
}
