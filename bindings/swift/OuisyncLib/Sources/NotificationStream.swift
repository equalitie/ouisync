import Foundation

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
        rx = Rx(bufferingPolicy: Tx.BufferingPolicy.bufferingOldest(1), { tx = $0 })
        self.rx_iter = rx.makeAsyncIterator()
        self.state = state
        state.registrations[subscriptionId] = tx
    }

    public func next() async -> ()? {
        return await rx_iter.next()
    }

    deinit {
        state.registrations.removeValue(forKey: subscriptionId)
    }
}
