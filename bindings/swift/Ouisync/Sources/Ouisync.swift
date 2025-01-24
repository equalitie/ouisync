public extension Client {
    // MARK: repository
    var storeDir: String { get async throws {
        try await invoke("repository_get_store_dir").stringValue.orThrow
    } }
    func setStoreDir(to path: String) async throws {
        try await invoke("repository_set_store_dir", with: .string(path))
    }

    var runtimeId: String { get async throws {
        try await invoke("network_get_runtime_id").stringValue.orThrow
    } }

    @available(*, deprecated, message: "Not supported on darwin")
    var mountRoot: String { get async throws {
        try await invoke("repository_get_mount_root").stringValue.orThrow
    } }

    @available(*, deprecated, message: "Not supported on darwin")
    func setMountRoot(to path: String) async throws {
        try await invoke("repository_set_mount_root", with: .string(path))
    }

    // MARK: network
    /// Initializes library network stack using the provided config.
    func initNetwork(bindTo addresses: [String] = [],
                     portForwarding: Bool = false,
                     localDiscovery: Bool = false) async throws {
        try await invoke("network_init", with: ["bind": .array(addresses.map { .string($0) }),
                                                "port_forwarding_enabled": .bool(portForwarding),
                                                "local_discovery_enabled": .bool(localDiscovery)])
    }

    var listenerAddrs: [String] { get async throws {
        try await invoke("network_get_listener_addrs").arrayValue.orThrow.map {
            try $0.stringValue.orThrow
        }
    } }
    /// Binds network to the specified addresses.
    func bindNetwork(to addresses: [String]) async throws {
        try await invoke("network_bind", with: .array(addresses.map { .string($0) }))
    }

    /// Is port forwarding (UPnP) enabled?
    var portForwarding: Bool { get async throws {
        try await invoke("network_is_port_forwarding_enabled").boolValue.orThrow
    } }
    /// Enable/disable port forwarding (UPnP)
    func setPortForwarding(enabled: Bool) async throws {
        try await invoke("network_set_port_forwarding_enabled", with: .bool(enabled))
    }

    /// Is local discovery enabled?
    var localDiscovery: Bool { get async throws {
        try await invoke("network_is_local_discovery_enabled").boolValue.orThrow
    } }
    /// Enable/disable local discovery
    func setLocalDiscovery(enabled: Bool) async throws {
        try await invoke("network_set_local_discovery_enabled", with: .bool(enabled))
    }

    nonisolated var networkEvents: AsyncThrowingMapSequence<Subscription, NetworkEvent> {
        subscribe(to: "network").map { try NetworkEvent(rawValue: $0.uint8Value.orThrow).orThrow }
    }

    var currentProtocolVersion: UInt64 { get async throws {
        try await invoke("network_get_current_protocol_version").uint64Value.orThrow
    } }

    var highestObservedProtocolVersion: UInt64 { get async throws {
        try await invoke("network_get_highest_seen_protocol_version").uint64Value.orThrow
    } }

    var natBehavior: String { get async throws {
        try await invoke("network_get_nat_behavior").stringValue.orThrow
    } }

    var externalAddressV4: String { get async throws {
        try await invoke("network_get_external_addr_v4").stringValue.orThrow
    } }

    var externalAddressV6: String { get async throws {
        try await invoke("network_get_external_addr_v6").stringValue.orThrow
    } }

    var networkStats: NetworkStats { get async throws {
        try await NetworkStats(invoke("network_stats"))
    } }

    // MARK: peers
    var peers: [PeerInfo] { get async throws {
        try await invoke("network_get_peers").arrayValue.orThrow.map { try PeerInfo($0) }
    } }

    // user provided
    var userProvidedPeers: [String] { get async throws {
        try await invoke("network_get_user_provided_peers").arrayValue.orThrow.map {
            try $0.stringValue.orThrow
        }
    } }

    func addUserProvidedPeers(from: [String]) async throws {
        try await invoke("network_add_user_provided_peers", with: .array(from.map { .string($0) }))
    }

    func removeUserProvidedPeers(from: [String]) async throws {
        try await invoke("network_remove_user_provided_peers", with: .array(from.map { .string($0) }))
    }

    // StateMonitor get rootStateMonitor => StateMonitor.getRoot(_client);
    // StateMonitor? get stateMonitor => StateMonitor.getRoot(_client)
    //      .child(MonitorId.expectUnique("Repositories"))
    //      .child(MonitorId.expectUnique(_path));
}
