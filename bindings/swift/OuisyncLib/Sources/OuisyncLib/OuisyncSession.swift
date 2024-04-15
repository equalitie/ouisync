//
//  Session.swift
//
//
//  Created by Peter Jankuliak on 15/04/2024.
//

import Foundation

public class OuisyncSession {
    // Used to send and receive messages from the Ouisync library
    let librarySender: OuisyncLibrarySenderProtocol

    var nextMessageId: MessageId = 0
    var pendingResponses: [MessageId: CheckedContinuation<Response, any Error>] = [:]
    var state: NotificationStream.State = NotificationStream.State()

    public init(_ libraryClient: OuisyncLibrarySenderProtocol) {
        self.librarySender = libraryClient
    }

    public func listRepositories() async throws -> [UInt64] {
        let response = try await sendRequest(MessageRequest.listRepositories(generateMessageId()));
        return response.toUInt64Array()!
    }

    public func getRepositoryName(_ handle: RepositoryHandle) async throws -> String {
        let response = try await sendRequest(MessageRequest.getRepositoryName(generateMessageId(), handle));
        let data = response.toData()!
        return String(decoding: data, as: UTF8.self)
    }

    public func subscribeToRepositoryListChange() async throws -> NotificationStream {
        let messageId = generateMessageId()
        let stream = NotificationStream(state)
        let _ = try await sendRequest(MessageRequest.subscribeToRepositoryListChange(messageId));
        return stream
    }

    func sendRequest(_ request: MessageRequest) async throws -> Response {
        async let onResponse = withCheckedThrowingContinuation { continuation in
            pendingResponses[request.messageId] = continuation
        }

        sendDataToOuisyncLib(request.serialize());

        return try await onResponse
    }

    func generateMessageId() -> MessageId {
        let messageId = nextMessageId
        nextMessageId += 1
        return messageId
    }

    func sendDataToOuisyncLib(_ data: [UInt8]) {
        librarySender.sendDataToOuisyncLib(data);
    }

    public func onReceiveDataFromOuisyncLib(_ data: [UInt8]) {
        let maybe_message = IncomingMessage.deserialize(data)

        guard let message = maybe_message else {
            NSLog(":::: 😡 Failed to parse incoming message from OuisyncLib \(data)")
            return
        }

        NSLog(":::: 🙂 Received message from OuisyncLib \(message)")

        switch message.payload {
        case .response(let response):
            handleResponse(message.messageId, response)
        case .notification(let notification):
            handleNotification(message.messageId, notification)
        case .error(let error):
            handleError(message.messageId, error)
        }
    }

    func handleResponse(_ messageId: MessageId, _ response: Response) {
        guard let pendingResponse = pendingResponses.removeValue(forKey: messageId) else {
            NSLog(":::: 😡 Failed to match response to a request")
            return
        }
        pendingResponse.resume(returning: response)
    }

    func handleNotification(_ messageId: MessageId, _ response: OuisyncNotification) {
        for tx in state.registrations.values {
            tx.yield(response)
        }
    }

    func handleError(_ messageId: MessageId, _ response: ErrorResponse) {
        guard let pendingResponse = pendingResponses.removeValue(forKey: messageId) else {
            NSLog(":::: 😡 Failed to match response to a request")
            return
        }
        pendingResponse.resume(throwing: response)
    }
}

public protocol OuisyncLibrarySenderProtocol {
    func sendDataToOuisyncLib(_ data: [UInt8])
}

public class NotificationStream {
    typealias Id = UInt64
    typealias Rx = AsyncStream<OuisyncNotification>
    typealias RxIter = Rx.AsyncIterator
    typealias Tx = Rx.Continuation

    class State {
        var registrations: [Id: Tx] = [:]
    }

    static var nextId: Id = 0
    let id: Id
    let rx: Rx
    var rx_iter: RxIter
    var state: State

    init(_ state: State) {
        id = NotificationStream.nextId;
        NotificationStream.nextId += 1

        var tx: Tx!
        rx = Rx { tx = $0 }
        self.rx_iter = rx.makeAsyncIterator()

        self.state = state

        state.registrations[id] = tx
    }

    public func next() async -> OuisyncNotification? {
        return await rx_iter.next()
    }

    deinit {
        state.registrations.removeValue(forKey: id)
    }
}

