import Foundation
import MessagePack


public extension Client {
    var root: StateMonitor { StateMonitor(self, []) }
}


public class StateMonitor {
    public struct Id: Equatable, Comparable {
        let name: String
        let disambiguator: UInt64
        public var description: String { "\(name):\(disambiguator)" }

        public static func < (lhs: Self, rhs: Self) -> Bool {
            lhs.name == rhs.name ? lhs.disambiguator < lhs.disambiguator : lhs.name < rhs.name
        }

        init(_ str: String) throws {
            guard let match = str.lastIndex(of: ":") else { throw OuisyncError.InvalidData }
            name = String(str[..<match])
            guard let dis = UInt64(str[str.index(after: match)...]) else { throw OuisyncError.InvalidData }
            disambiguator = dis
        }
    }

    let client: Client
    public let path: [Id]
    private(set) public var values = [String: String]()
    private(set) public var children = [StateMonitor]()
    init(_ client: Client, _ path: [Id]) {
        self.client = client
        self.path = path
    }
}


public extension StateMonitor {
    var changes: AsyncThrowingMapSequence<Client.Subscription, Void> {
        client.subscribe(to: "state_monitor",
                         with: .array(path.map { .string($0.description) })).map { _ in () }
    }

    func load() async throws -> Bool {
        let res = try await client.invoke("state_monitor_get",
                                          with: .array(path.map { .string($0.description) }))
        if case .nil = res { return false }
        guard case .array(let arr) = res, arr.count == 2 else { throw OuisyncError.InvalidData }

        values = try .init(uniqueKeysWithValues: arr[0].dictionaryValue.orThrow.map {
            try ($0.key.stringValue.orThrow, $0.value.stringValue.orThrow)
        })
        children = try arr[1].arrayValue.orThrow.lazy.map {
            try StateMonitor(client, path + [Id($0.stringValue.orThrow)])
        }
        return true
    }
}
