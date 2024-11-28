import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import '../client.dart';
import 'length_delimited.dart';
import 'message_matcher.dart';

class SocketClient extends Client {
  final Socket _socket;
  final StreamSubscription<Uint8List> _subscription;
  final MessageMatcher _messageMatcher;

  SocketClient._(this._socket, this._subscription, this._messageMatcher);

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
        final messageMatcher = MessageMatcher();
        final subscription = socket
            .transform(LengthDelimitedCodec())
            .listen(messageMatcher.receive);

        return SocketClient._(socket, subscription, messageMatcher);
      } on SocketException catch (e) {
        lastException = e;
        delay = _minDuration(delay * 2, maxDelay);
      }
    }
  }

  @override
  Future<T> invoke<T>(String method, [Object? args]) async {
    final (message, response) = _messageMatcher.send(method, args);
    _socket.add(encode(message));
    return await response;
  }

  @override
  Future<void> close() async {
    await _subscription.cancel();
    await _socket.close();
  }

  @override
  void subscribe(int id, StreamSink<Object?> sink) {
    _messageMatcher.subscribe(id, sink);
  }

  @override
  void unsubscribe(int id) {
    _messageMatcher.unsubscribe(id);
  }
}

_minDuration(Duration a, Duration b) => (a < b) ? a : b;
