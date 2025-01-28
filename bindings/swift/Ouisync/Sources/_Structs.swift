import MessagePack
import Foundation


extension Optional {
    /// A softer version of unwrap (!) that throws `OuisyncError.InvalidData` instead of crashing
    var orThrow: Wrapped { get throws {
        guard let self else { throw OuisyncError.InvalidData }
        return self
    } }
}


public struct NetworkStats {
    public let bytesTx: UInt64
    public let bytesRx: UInt64
    public let throughputTx: UInt64
    public let throughputRx: UInt64

    init(_ messagePack: MessagePackValue) throws {
        guard let arr = messagePack.arrayValue, arr.count == 4 else { throw OuisyncError.InvalidData }
        bytesTx = try arr[0].uint64Value.orThrow
        bytesRx = try arr[1].uint64Value.orThrow
        throughputTx = try arr[2].uint64Value.orThrow
        throughputRx = try arr[3].uint64Value.orThrow
    }
}


public struct PeerInfo {
    public let addr: String
    public let source: PeerSource
    public let state: PeerStateKind
    public let runtimeId: String?
    public let stats: NetworkStats

    init(_ messagePack: MessagePackValue) throws {
        guard let arr = messagePack.arrayValue, arr.count == 4 else { throw OuisyncError.InvalidData }
        addr = try arr[0].stringValue.orThrow
        source = try PeerSource(rawValue: arr[1].uint8Value.orThrow).orThrow
        if let kind = arr[2].uint8Value {
            state = try PeerStateKind(rawValue: kind).orThrow
            runtimeId = nil
        } else if let arr = arr[2].arrayValue, arr.count >= 2 {
            state = try PeerStateKind(rawValue: arr[0].uint8Value.orThrow).orThrow
            runtimeId = try arr[1].dataValue.orThrow.map({ String(format: "%02hhx", $0) }).joined()
            // FIXME: arr[3] seems to be an undocumented timestamp in milliseconds
        } else {
            throw OuisyncError.InvalidData
        }
        stats = try NetworkStats(arr[3])
    }
}


public enum EntryType: Equatable {
    case File(_ version: Data)
    case Directory(_ version: Data)

    init?(_ value: MessagePackValue) throws {
        switch value {
        case .nil:
            return nil
        case .map(let fields):
            guard fields.count == 1, let pair = fields.first, let version = pair.value.dataValue,
                  version.count == 32 else { throw OuisyncError.InvalidData }
            switch try pair.key.stringValue.orThrow {
            case "File": self = .File(version)
            case "Directory": self = .Directory(version)
            default: throw OuisyncError.InvalidData
            }
        default: throw OuisyncError.InvalidData
        }
    }
}


public struct SyncProgress {
    public let value: UInt64
    public let total: UInt64

    init(_ messagePack: MessagePackValue) throws {
        guard let arr = messagePack.arrayValue, arr.count == 2 else { throw OuisyncError.InvalidData }
        value = try arr[0].uint64Value.orThrow
        total = try arr[1].uint64Value.orThrow
    }
}


// TODO: automatically generate these enums after https://github.com/mozilla/cbindgen/issues/1039
public enum AccessMode: UInt8 {
    /// Repository is neither readable not writtable (but can still be synced).
    case Blind = 0
    /// Repository is readable but not writtable.
    case Read = 1
    /// Repository is both readable and writable.
    case Write = 2
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
