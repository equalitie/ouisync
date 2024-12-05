import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:ouisync/ouisync.dart';

import '../client.dart';
import 'length_delimited_codec.dart';
import 'message_codec.dart';

class SocketClient extends Client {
  final Socket _socket;
  StreamSubscription<Uint8List>? _socketSubscription;
  int _nextMessageId = 0;
  final _responses = <int, Completer<Object?>>{};
  final _notifications = <int, StreamSink<Object?>>{};

  SocketClient._(this._socket) {
    _socketSubscription =
        _socket.transform(LengthDelimitedCodec()).listen(_receive);
  }

  static Future<SocketClient> connect(
    String path, {
    Duration? timeout,
    Duration minDelay = const Duration(milliseconds: 50),
    Duration maxDelay = const Duration(seconds: 1),
  }) async {
    final addr = InternetAddress(path, type: InternetAddressType.unix);

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
        final socket = await Socket.connect(addr, 0);
        return SocketClient._(socket);
      } on SocketException catch (e) {
        lastException = e;
        delay = _minDuration(delay * 2, maxDelay);
        await Future.delayed(delay);
      }
    }
  }

  @override
  Future<T> invoke<T>(String method, [Object? args]) async {
    final id = _nextMessageId++;
    return await _invokeWithMessageId(id, method, args);
  }

  @override
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

  @override
  Future<void> close() async {
    await _socketSubscription?.cancel();
    await _socket.close();
  }

  Future<T> _invokeWithMessageId<T>(int id, String method, Object? args) async {
    final completer = Completer();
    final response = completer.future;

    _responses[id] = completer;

    final bytes = encodeLengthDelimited(encodeMessage(id, method, args));
    _socket.add(bytes);

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
