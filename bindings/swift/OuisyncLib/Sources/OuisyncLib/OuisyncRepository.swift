//
//  OuisyncRepository.swift
//  
//
//  Created by Peter Jankuliak on 15/04/2024.
//

import Foundation

public class OuisyncRepository {
    let handle: RepositoryHandle
    let session: OuisyncSession

    init(_ handle: RepositoryHandle, _ session: OuisyncSession) {
        self.handle = handle
        self.session = session
    }

    func getName() async throws {
        
    }

}
