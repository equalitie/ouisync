import 'dart:async';

export 'internal/direct_client.dart';
export 'internal/channel_client.dart';
export 'internal/socket_client.dart';

abstract class Client {
  Future<T> invoke<T>(String method, [Object? args]);
  void subscribe(int id, StreamSink<Object?> sink);
  void unsubscribe(int id);
  Future<void> close();
}

class Subscription {
  final Client _client;
  final StreamController<Object?> _controller;
  final String _name;
  final Object? _arg;
  int _id = 0;
  _SubscriptionState _state = _SubscriptionState.idle;

  Subscription(this._client, this._name, this._arg)
      : _controller = StreamController.broadcast() {
    _controller.onListen = () => _switch(_SubscriptionState.subscribing);
    _controller.onCancel = () => _switch(_SubscriptionState.unsubscribing);
  }

  Stream<Object?> get stream => _controller.stream;

  Future<void> close() async {
    if (_controller.hasListener) {
      return await _controller.close();
    }
  }

  Future<void> _switch(_SubscriptionState target) async {
    switch (_state) {
      case _SubscriptionState.idle:
        _state = target;
        break;
      case _SubscriptionState.subscribing:
      case _SubscriptionState.unsubscribing:
        _state = target;
        return;
    }

    while (true) {
      final state = _state;

      switch (state) {
        case _SubscriptionState.idle:
          return;
        case _SubscriptionState.subscribing:
          await _subscribe();
          break;
        case _SubscriptionState.unsubscribing:
          await _unsubscribe();
          break;
      }

      if (_state == state) {
        _state = _SubscriptionState.idle;
      }
    }
  }

  Future<void> _subscribe() async {
    if (_id != 0) {
      return;
    }

    try {
      _id = await _client.invoke('${_name}_subscribe', _arg) as int;

      // This subscription might have been `close`d in the meantime. Don't register the sink so
      // that if a notification is still received from the backend it won't get added to the now
      // closed controller (which would throw an exception).
      if (_controller.isClosed) {
        return;
      }

      _client.subscribe(_id, _controller.sink);
    } catch (e) {
      print('failed to subscribe to $_name: $e');
    }
  }

  Future<void> _unsubscribe() async {
    if (_id == 0) {
      return;
    }

    _client.unsubscribe(_id);

    try {
      await _client.invoke('unsubscribe', _id);
    } catch (e) {
      print('failed to unsubscribe from $_name: $e');
    }

    _id = 0;
  }
}

enum _SubscriptionState {
  idle,
  subscribing,
  unsubscribing,
}
