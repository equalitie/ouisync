/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

import Foundation
import MessagePack
import OuisyncLibFFI

public class OuisyncSession {
    let configsPath: String
    let logsPath: String

    let client: OuisyncClient

    var nextMessageId: MessageId = 0
    var pendingResponses: [MessageId: CheckedContinuation<Response, any Error>] = [:]
    var notificationSubscriptions: NotificationStream.State = NotificationStream.State()

    public init(_ configsPath: String, _ logsPath: String, _ ffi: OuisyncFFI) throws {
        self.configsPath = configsPath
        self.logsPath = logsPath

        client = try OuisyncClient.create(configsPath, logsPath, ffi)
        client.onReceiveFromBackend = { [weak self] data in
            self?.onReceiveDataFromOuisyncLib(data)
        }
    }

    public func connectNewClient() throws -> OuisyncClient {
        return try OuisyncClient.create(configsPath, logsPath, client.ffi)
    }

    // Can be called from a separate thread.
    public func invoke(_ requestMsg: OuisyncRequestMessage) async -> OuisyncResponseMessage {
        let responsePayload: OuisyncResponsePayload

        do {
            responsePayload = .response(try await sendRequest(requestMsg.request))
        } catch let e as OuisyncError {
            responsePayload = .error(e)
        } catch let e {
            fatalError("Unhandled exception in OuisyncSession.invoke: \(e)")
        }

        return OuisyncResponseMessage(requestMsg.messageId, responsePayload)
    }

    public func listRepositories() async throws -> [OuisyncRepository] {
        let response = try await sendRequest(OuisyncRequest.listRepositories());
        let handles = response.toUInt64Array()
        return handles.map({ OuisyncRepository($0, self) })
    }

    public func subscribeToRepositoryListChange() async throws -> NotificationStream {
        let subscriptionId = try await sendRequest(OuisyncRequest.subscribeToRepositoryListChange()).toUInt64();
        return NotificationStream(subscriptionId, notificationSubscriptions)
    }

    public func subscribeToRepositoryChange(_ repo: RepositoryHandle) async throws -> NotificationStream {
        let subscriptionId = try await sendRequest(OuisyncRequest.subscribeToRepositoryChange(repo)).toUInt64();
        return NotificationStream(subscriptionId, notificationSubscriptions)
    }

    // Can be called from a separate thread.
    internal func sendRequest(_ request: OuisyncRequest) async throws -> Response {
        let messageId = generateMessageId()

        async let onResponse = withCheckedThrowingContinuation { [weak self] continuation in
            guard let session = self else { return }

            synchronized(session) {
                session.pendingResponses[messageId] = continuation
                let data = OuisyncRequestMessage(messageId, request).serialize()
                session.client.sendToBackend(data)
            }
        }

        return try await onResponse
    }

    // Can be called from a separate thread.
    fileprivate func generateMessageId() -> MessageId {
        synchronized(self) {
            let messageId = nextMessageId
            nextMessageId += 1
            return messageId
        }
    }

    // Use this function to pass data from the backend.
    // It may be called from a separate thread.
    public func onReceiveDataFromOuisyncLib(_ data: [UInt8]) {
        let maybe_message = OuisyncResponseMessage.deserialize(data)

        guard let message = maybe_message else {
            let hex = data.map({String(format:"%02x", $0)}).joined(separator: ",")
            // Likely cause is a version mismatch between the backend (Rust) and frontend (Swift) code.
            fatalError("Failed to parse incoming message from OuisyncLib [\(hex)]")
        }

        switch message.payload {
        case .response(let response):
            handleResponse(message.messageId, response)
        case .notification(let notification):
            handleNotification(message.messageId, notification)
        case .error(let error):
            handleError(message.messageId, error)
        }
    }

    fileprivate func handleResponse(_ messageId: MessageId, _ response: Response) {
        let maybePendingResponse = synchronized(self) { pendingResponses.removeValue(forKey: messageId) }

        guard let pendingResponse = maybePendingResponse else {
            fatalError("❗ Failed to match response to a request. messageId:\(messageId), repsponse:\(response) ")
        }

        pendingResponse.resume(returning: response)
    }

    fileprivate func handleNotification(_ messageId: MessageId, _ response: OuisyncNotification) {
        let maybeTx = synchronized(self) { notificationSubscriptions.registrations[messageId] }

        if let tx = maybeTx {
            tx.yield(())
        } else {
            NSLog("❗ Received unsolicited notification")
        }
    }

    fileprivate func handleError(_ messageId: MessageId, _ response: OuisyncError) {
        let maybePendingResponse = synchronized(self) { pendingResponses.removeValue(forKey: messageId) }

        guard let pendingResponse = maybePendingResponse else {
            fatalError("❗ Failed to match error response to a request. messageId:\(messageId), response:\(response)")
        }

        pendingResponse.resume(throwing: response)
    }

}

fileprivate func synchronized<T>(_ lock: AnyObject, _ closure: () throws -> T) rethrows -> T {
    objc_sync_enter(lock)
    defer { objc_sync_exit(lock) }
    return try closure()
}

public class NotificationStream {
    typealias Id = UInt64
    typealias Rx = AsyncStream<()>
    typealias RxIter = Rx.AsyncIterator
    typealias Tx = Rx.Continuation

    class State {
        var registrations: [Id: Tx] = [:]
    }

    let subscriptionId: Id
    let rx: Rx
    var rx_iter: RxIter
    var state: State

    init(_ subscriptionId: Id, _ state: State) {
        self.subscriptionId = subscriptionId
        var tx: Tx!
        rx = Rx (bufferingPolicy: Tx.BufferingPolicy.bufferingOldest(1), { tx = $0 })
        self.rx_iter = rx.makeAsyncIterator()

        self.state = state

        state.registrations[subscriptionId] = tx
    }

    public func next() async -> ()? {
        return await rx_iter.next()
    }

    deinit {
        // TODO: We should have a `close() async` function where we unsubscripbe
        // from the notifications.
        state.registrations.removeValue(forKey: subscriptionId)
    }
}

