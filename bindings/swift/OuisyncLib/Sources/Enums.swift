// TODO: automatically generate this file after https://github.com/mozilla/cbindgen/issues/1039
public enum AccessMode: UInt8 {
    /// Repository is neither readable not writtable (but can still be synced).
    case Blind = 0
    /// Repository is readable but not writtable.
    case Read = 1
    /// Repository is both readable and writable.
    case Write = 2
}

public enum EntryType: UInt8 {
    case File = 1
    case Directory = 2
}

public enum NetworkEvent: UInt8 {
    /// A peer has appeared with higher protocol version than us. Probably means we are using
    /// outdated library. This event can be used to notify the user that they should update the app.
    case ProtocolVersionMismatch = 0
    /// The set of known peers has changed (e.g., a new peer has been discovered)
    case PeerSetChange = 1
}

public enum PeerSource: UInt8 {
    /// Explicitly added by the user.
    case UserProvided = 0
    /// Peer connected to us.
    case Listener = 1
    /// Discovered on the Local Discovery.
    case LocalDiscovery = 2
    /// Discovered on the DHT.
    case Dht = 3
    /// Discovered on the Peer Exchange.
    case PeerExchange = 4
}

public enum PeerStateKind: UInt8 {
    /// The peer is known (discovered or explicitly added by the user) but we haven't started
    /// establishing a connection to them yet.
    case Known = 0
    /// A connection to the peer is being established.
    case Connecting = 1
    /// The peer is connected but the protocol handshake is still in progress.
    case Handshaking = 2
    /// The peer connection is active.
    case Active = 3
}
