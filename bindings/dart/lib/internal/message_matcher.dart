import 'dart:async';
import 'dart:typed_data';
import 'dart:collection';
import 'package:msgpack_dart/msgpack_dart.dart';

import '../client.dart';
import '../errors.dart';
import '../ouisync.dart' show Error;
import '../bindings.dart' show ErrorCode;

typedef Sender = Future<void> Function(Uint8List);

// Matches requests to responses.
class MessageMatcher {
  var _nextMessageId = 0;
  final _responses = HashMap<int, Completer<Object?>>();
  final _subscriptions = Subscriptions();
  bool _isClosed = false;

  Subscriptions subscriptions() => _subscriptions;

  // Throws if the response is an error or if sending the message fails.
  Future<T> sendAndAwaitResponse<T>(String method, Object? args, Sender send) async {
    final id = _nextMessageId++;
    final completer = Completer();

    _responses[id] = completer;

    final request = {method: args};

    // DEBUG
    //print('send: id: $id, request: $request');

    try {
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
        
      if (_isClosed) {
        throw SessionClosed();
      }

      await send(message);

      return await completer.future as T;
    } finally {
      _responses.remove(id);
    }
  }

  void handleResponse(Uint8List bytes) {
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
      _subscriptions.handle(id, message['notification']);
    } else {
      final responseCompleter = _responses.remove(id);
      if (responseCompleter != null) {
        _handleInvalidResponse(responseCompleter);
      }
    }
  }

  void close() {
    _isClosed = true;
    _subscriptions.close();
    final error = SessionClosed();
    for (var i in _responses.values) {
      i.completeError(error);
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

    final error = Error(ErrorCode.decode(code), message);
    completer.completeError(error);
  }

  void _handleInvalidResponse(Completer<Object?> completer) {
    final error = Exception('invalid response');
    completer.completeError(error);
  }
}
