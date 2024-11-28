import 'dart:async';
import 'dart:typed_data';
import 'dart:collection';
import 'package:msgpack_dart/msgpack_dart.dart';

import '../errors.dart';
import '../ouisync.dart' show OuisyncException, ErrorCode;

typedef Sender = Future<void> Function(Uint8List);

// Matches requests to responses.
class MessageMatcher {
  var _nextMessageId = 0;
  final _responses = HashMap<int, Completer<Object?>>();
  final _sinks = HashMap<int, StreamSink<Object?>>();
  bool _isClosed = false;

  (Uint8List, Future<T>) send<T>(
    String method,
    Object? args,
  ) {
    if (_isClosed) {
      throw SessionClosed();
    }

    final id = _nextMessageId++;
    final completer = Completer();

    _responses[id] = completer;

    final request = {method: args};

    // DEBUG
    //print('send: id: $id, request: $request');

    // Message format:
    //
    // +-------------------------------------+-------------------------------------------+
    // | id (big endian 64 bit unsigned int) | request (messagepack encoded byte string) |
    // +-------------------------------------+-------------------------------------------+
    //
    // This allows the server to decode the id even if the request is malformed so it can send
    // error response back.
    final message = (BytesBuilder()
          ..add((ByteData(8)..setUint64(0, id)).buffer.asUint8List())
          ..add(serialize(request)))
        .takeBytes();

    return (message, completer.future.then((value) => value as T));
  }

  void receive(Uint8List bytes) {
    if (bytes.length < 8) {
      return;
    }

    final id = bytes.buffer.asByteData().getUint64(0);
    final message = deserialize(bytes.sublist(8));

    // DEBUG
    //print('recv: id: $id, message: $message');

    if (message is! Map) {
      return;
    }

    final isSuccess = message.containsKey('success');
    final isFailure = message.containsKey('failure');
    final isNotification = message.containsKey('notification');

    if (isSuccess || isFailure) {
      final responseCompleter = _responses.remove(id);
      if (responseCompleter == null) {
        print('unsolicited response');
        return;
      }

      if (isSuccess) {
        _handleResponseSuccess(responseCompleter, message['success']);
      } else if (isFailure) {
        _handleResponseFailure(responseCompleter, message['failure']);
      }
    } else if (isNotification) {
      _handleNotification(id, message['notification']);
    } else {
      final responseCompleter = _responses.remove(id);
      if (responseCompleter != null) {
        _handleInvalidResponse(responseCompleter);
      }
    }
  }

  void subscribe(int subscriptionId, StreamSink<Object?> sink) {
    _sinks[subscriptionId] = sink;
  }

  void unsubscribe(int subscriptionId) {
    _sinks.remove(subscriptionId);
  }

  void close() {
    _isClosed = true;

    for (final sink in _sinks.values) {
      sink.close();
    }

    _sinks.clear();

    final error = SessionClosed();

    for (final completer in _responses.values) {
      completer.completeError(error);
    }

    _responses.clear();
  }

  void _handleResponseSuccess(Completer<Object?> completer, Object? payload) {
    if (payload == "none") {
      completer.complete(null);
      return;
    }

    if (payload is Map && payload.length == 1) {
      completer.complete(payload.entries.single.value);
    } else {
      _handleInvalidResponse(completer);
    }
  }

  void _handleResponseFailure(Completer<Object?> completer, Object? payload) {
    if (payload is! List) {
      _handleInvalidResponse(completer);
      return;
    }

    final code = payload[0];
    final message = payload[1];

    if (code is! int || message is! String) {
      _handleInvalidResponse(completer);
      return;
    }

    final error = OuisyncException(ErrorCode.decode(code), message);
    completer.completeError(error);
  }

  void _handleInvalidResponse(Completer<Object?> completer) {
    final error = Exception('invalid response');
    completer.completeError(error);
  }

  void _handleNotification(int subscriptionId, Object? payload) {
    final sink = _sinks[subscriptionId];

    if (sink == null) {
      print('unsolicited notification');
      return;
    }

    try {
      if (payload is String) {
        sink.add(null);
      } else if (payload is Map && payload.length == 1) {
        sink.add(payload.entries.single.value);
      } else {
        final error = Exception('invalid notification');
        sink.addError(error);
      }
    } catch (error) {
      // We can get here if the `_controller` has been `close`d but the
      // `_controller.onCancel` has not yet been executed (that's where the
      // `Subscription` is removed from `_subscriptions`). We just ignore that
      // error.
    }
  }
}
