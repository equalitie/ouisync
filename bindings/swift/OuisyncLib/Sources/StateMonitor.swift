import Foundation
import MessagePack

public struct MonitorId: Hashable {
    public let name: String
    public let disambiguator: Int64

    public static func expectUnique(_ name: String) -> MonitorId {
        MonitorId(name: name, disambiguator: 0)
    }

    internal func encodeToMsgPack() -> MessagePackValue {
        .string("\(name):\(disambiguator)")
    }

    internal static func decodeFromMsgPack(_ v: MessagePackValue) -> MonitorId? {
        guard case .string(let s) = v else { return nil }
        guard let colon = s.lastIndex(of: ":") else { return nil }
        let namePart = String(s[..<colon])
        guard let disambiguator = Int64(s[s.index(after: colon)...]) else { return nil }
        return MonitorId(name: namePart, disambiguator: disambiguator)
    }
}

public struct StateMonitorNode {
    public let values: [String: String]
    public let children: [MonitorId]

    // Decode from array [map<str,str>, [MonitorId...]]
    internal static func decodeFromMsgPack(_ v: MessagePackValue) -> StateMonitorNode? {
        guard case .array(let arr) = v, arr.count == 2 else { return nil }
        guard case .map(let valuesMap) = arr[0] else { return nil }
        var values: [String: String] = [:]
        for (k, v) in valuesMap {
            guard case .string(let ks) = k, case .string(let vs) = v else { return nil }
            values[ks] = vs
        }
        guard case .array(let childrenArr) = arr[1] else { return nil }
        let children = childrenArr.compactMap { MonitorId.decodeFromMsgPack($0) }
        guard children.count == childrenArr.count else { return nil }
        return StateMonitorNode(values: values, children: children)
    }

    // Encode as array [map<str,str>, [MonitorId...]]
    internal func encodeToMsgPack() -> MessagePackValue {
        let valuesMap = MessagePackValue.map(
            Dictionary(uniqueKeysWithValues: values.map { (.string($0.key), MessagePackValue.string($0.value)) })
        )
        let childrenArr = MessagePackValue.array(children.map { $0.encodeToMsgPack() })
        return .array([valuesMap, childrenArr])
    }
}
