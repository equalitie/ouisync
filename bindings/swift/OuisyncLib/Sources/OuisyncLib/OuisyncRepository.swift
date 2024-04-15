//
//  OuisyncRepository.swift
//  
//
//  Created by Peter Jankuliak on 15/04/2024.
//

import Foundation

public class OuisyncRepository: Hashable, CustomStringConvertible {
    let session: OuisyncSession
    public let handle: RepositoryHandle

    public init(_ handle: RepositoryHandle, _ session: OuisyncSession) {
        self.handle = handle
        self.session = session
    }

    public func getName() async throws -> String {
        let response = try await session.sendRequest(MessageRequest.getRepositoryName(handle));
        let data = response.toData()!
        return String(decoding: data, as: UTF8.self)
    }

    public static func == (lhs: OuisyncRepository, rhs: OuisyncRepository) -> Bool {
        return lhs.session === rhs.session && lhs.handle == rhs.handle
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(ObjectIdentifier(session))
        hasher.combine(handle)
    }

    public var description: String {
        return "OuisyncRepository(handle: \(handle))"
    }
}
