import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'exception.dart';
import 'internal/length_delimited_codec.dart';
import 'internal/message_codec.dart';
import 'internal/transport.dart';

class Client {
  final Transport _transport;
  StreamSubscription<Uint8List>? _streamSubscription;
  int _nextMessageId = 0;
  final _responses = <int, Completer<Object?>>{};
  final _notifications = <int, StreamSink<Object?>>{};

  Client._(this._transport) {
    _streamSubscription =
        _transport.stream.transform(LengthDelimitedCodec()).listen(_receive);
  }

  static Future<Client> connect(
    String path, {
    Duration? timeout,
    Duration minDelay = const Duration(milliseconds: 50),
    Duration maxDelay = const Duration(seconds: 1),
  }) async {
    DateTime start = DateTime.now();
    Duration delay = minDelay;
    SocketException? lastException;

    while (true) {
      if (timeout != null) {
        if (DateTime.now().difference(start) >= timeout) {
          throw lastException ?? TimeoutException('connect timeout');
        }
      }

      try {
        final transport = await Transport.connect(path);
        return Client._(transport);
      } on SocketException catch (e) {
        lastException = e;
        delay = _minDuration(delay * 2, maxDelay);
        await Future.delayed(delay);
      }
    }
  }

  Future<T> invoke<T>(String method, [Object? args]) async {
    final id = _nextMessageId++;
    return await _invokeWithMessageId(id, method, args);
  }

  Stream<T> subscribe<T>(String method, Object? arg) {
    final controller = StreamController<Object?>();
    final id = _nextMessageId++;

    controller.onListen = () => unawaited(_onSubscriptionListen(
          id,
          method,
          arg,
          controller.sink,
        ));

    controller.onCancel = () => unawaited(_onSubscriptionCancel(id));

    return controller.stream.cast<T>();
  }

  Future<void> close() async {
    await _streamSubscription?.cancel();
    await _transport.close();
  }

  Future<T> _invokeWithMessageId<T>(int id, String method, Object? args) async {
    final completer = Completer();
    final response = completer.future;

    _responses[id] = completer;

    final bytes = encodeLengthDelimited(encodeMessage(id, method, args));
    _transport.sink.add(bytes);

    return await response;
  }

  void _receive(Uint8List bytes) {
    final message = decodeMessage(bytes);

    switch (message) {
      case MessageSuccess():
        final completer = _responses.remove(message.id);
        if (completer != null) {
          completer.complete(message.payload);
          return;
        }

        final controller = _notifications[message.id];
        if (controller != null) {
          controller.add(message.payload);
        }

      case MessageFailure():
        final completer = _responses.remove(message.id);
        if (completer != null) {
          completer.completeError(message.exception);
        }

      case MalformedPayload():
        final completer = _responses.remove(message.id);
        if (completer != null) {
          completer.completeError(InvalidData('invalid response'));
        }

      case MalformedMessage():
    }
  }

  Future<void> _onSubscriptionListen(
    int id,
    String method,
    Object? arg,
    StreamSink<Object?> sink,
  ) async {
    _notifications[id] = sink;

    try {
      await _invokeWithMessageId<void>(id, '${method}_subscribe', arg);
    } catch (e, st) {
      _notifications.remove(id);
      sink.addError(e, st);
    }
  }

  Future<void> _onSubscriptionCancel(int id) async {
    _notifications.remove(id);
    await invoke<void>('unsubscribe', id);
  }
}

_minDuration(Duration a, Duration b) => (a < b) ? a : b;
