import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

import 'package:crypto/crypto.dart';
import 'package:hex/hex.dart';

import '../generated/api.g.dart'
    show
        InvalidData,
        MessageId,
        PermissionDenied,
        Request,
        Response,
        ResponseNone,
        RequestCancel;
import 'length_delimited_codec.dart';
import 'message_codec.dart';

class Client {
  final _Connector _connector;

  _SocketState _state = const _Disconnected();

  var _nextMessageId = 0;
  final _responses = <int, Completer<Object?>>{};
  final _notifications = <int, StreamSink<Object?>>{};
  // The request that established each entry in `_notifications`, kept around so a
  // subscription can be transparently replayed against a new connection after the
  // underlying socket is lost (e.g. iOS tearing down sockets while backgrounded).
  final _requests = <int, Request>{};

  Client._(this._connector);

  static Future<Client> connect({
    required String configPath,
    Duration minReconnectDelay = const Duration(milliseconds: 50),
    Duration maxReconnectDelay = const Duration(seconds: 1),
  }) async {
    final (port, authKey) =
        await _readLocalEndpoint('$configPath/local_endpoint.conf');

    final connector = _Connector(
      port: port,
      authKey: authKey,
      minDelay: minReconnectDelay,
      maxDelay: maxReconnectDelay,
    );

    return Client._(connector);
  }

  Future<Response> invoke(Request request) async {
    final id = _nextMessageId++;
    return await _invokeWithMessageId(id, request);
  }

  Stream<Response> subscribe<T>(Request request) {
    final controller = StreamController<Response>();
    final id = _nextMessageId++;

    controller.onListen =
        () => unawaited(_onSubscriptionListen(id, request, controller.sink));

    controller.onCancel = () => unawaited(_onSubscriptionCancel(id));

    return controller.stream.cast();
  }

  /// Forcibly drops the current connection (if any) and reconnects, replaying any currently
  /// active subscriptions against the fresh connection. The connection already does this
  /// automatically when it detects it was lost (see `_receiveError`/`_receiveDone`); this is a
  /// manual escape hatch for callers that suspect the connection is wedged in some way that
  /// wasn't detected on its own (e.g. a long stretch of silence after the app resumes from the
  /// background) and want to force a clean reconnect without waiting for the next failed call.
  Future<void> reset() async {
    // ignore: avoid_print
    print('ouisync.Client: reset() requested');
    await _disconnect();
    _resubscribeAll();
  }

  Future<void> close() async {
    while (true) {
      final state = _state;

      switch (state) {
        case _Transitioning():
          await state.future;
        case _Disconnected():
          _state = _Closed();
          return;
        case _Connected():
          final completer = Completer();
          _state = _Transitioning(completer.future);

          await state.subscription.cancel();
          await state.socket.close();

          _state = _Closed();
          completer.complete();
          return;
        case _Closed():
          return;
      }
    }
  }

  Future<Response> _invokeWithMessageId(int id, Request request) async {
    final bytes = encodeLengthDelimited(encodeMessage(id, request));

    while (true) {
      final completer = Completer();
      final response = completer.future;
      _responses[id] = completer;

      final socket = await _ensureConnected();

      try {
        socket.add(bytes);
        return await response;
      } on SocketException {
        await _disconnect();
      }
    }
  }

  Future<Socket> _ensureConnected() async {
    while (true) {
      final state = _state;

      switch (state) {
        case _Transitioning():
          await state.future;
        case _Disconnected():
          final completer = Completer();
          _state = _Transitioning(completer.future);

          final (socket, stream) = await _connector.connect();
          final subscription = stream.transform(LengthDelimitedCodec()).listen(
                _receive,
                onError: _receiveError,
                onDone: _receiveDone,
              );

          _state = _Connected(socket, subscription);
          completer.complete();
        case _Connected():
          return state.socket;
        case _Closed():
          throw StateError('client closed');
      }
    }
  }

  Future<void> _disconnect() async {
    while (true) {
      final state = _state;
      switch (state) {
        case _Transitioning():
          await state.future;
        case _Disconnected():
          return;
        case _Connected():
          final completer = Completer();
          _state = _Transitioning(completer.future);

          await state.subscription.cancel();
          await state.socket.close();

          _state = _Disconnected();
          completer.complete();
          return;
        case _Closed():
          return;
      }
    }
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

        if (message.payload is ResponseNone) {
          // Stream closed by the service.
          final sink = _notifications.remove(message.id);
          _requests.remove(message.id);
          if (sink != null) {
            sink.close();
          }
        } else {
          final sink = _notifications[message.id];
          if (sink != null) {
            sink.add(message.payload);
          }
        }

      case MessageFailure():
        final completer = _responses.remove(message.id);
        if (completer != null) {
          completer.completeError(message.exception);
          return;
        }

        final sink = _notifications[message.id];
        if (sink != null) {
          sink.addError(message.exception);
        }

      case MalformedPayload():
        final completer = _responses.remove(message.id);
        final exception = InvalidData('invalid response');

        if (completer != null) {
          completer.completeError(exception);
          return;
        }

        final sink = _notifications[message.id];
        if (sink != null) {
          sink.addError(exception);
        }

      case MalformedMessage():
    }
  }

  void _receiveError(Object error, StackTrace st) {
    for (final completer in _responses.takeValues()) {
      completer.completeError(error, st);
    }

    _state = _Disconnected();
    _resubscribeAll();
  }

  void _receiveDone() {
    for (final completer in _responses.takeValues()) {
      completer.completeError(SocketException.closed());
    }

    _state = _Disconnected();
    _resubscribeAll();
  }

  // Losing the connection (e.g. the OS tearing down sockets while the app is backgrounded)
  // must not permanently kill already-active subscriptions: replay each one against the new
  // connection instead of erroring out its sink, so streams keep working transparently once
  // reconnected. `_onSubscriptionListen` re-registers the sink in `_notifications` as part of
  // resending the subscribe request, so clear the map first to avoid it looking already
  // subscribed.
  void _resubscribeAll() {
    final toResubscribe = Map.of(_notifications);
    _notifications.clear();

    // ignore: avoid_print
    print(
      'ouisync.Client: connection lost, resubscribing ${toResubscribe.length} '
      'active subscription(s): ${toResubscribe.keys.toList()}',
    );

    for (final entry in toResubscribe.entries) {
      final request = _requests[entry.key];
      if (request != null) {
        unawaited(_onSubscriptionListen(entry.key, request, entry.value));
      } else {
        // Should not happen (every entry in `_notifications` has a matching `_requests`
        // entry), but don't leak the sink if it ever does.
        entry.value.addError(StateError('lost subscription request'));
      }
    }
  }

  Future<void> _onSubscriptionListen(
    int id,
    Request request,
    StreamSink<Object?> sink,
  ) async {
    _notifications[id] = sink;
    _requests[id] = request;

    try {
      await _invokeWithMessageId(id, request);
      // ignore: avoid_print
      print('ouisync.Client: subscription $id (re)established: $request');
    } catch (e, st) {
      _notifications.remove(id);
      _requests.remove(id);
      // ignore: avoid_print
      print('ouisync.Client: subscription $id failed to (re)establish: $e');
      sink.addError(e, st);
    }
  }

  Future<void> _onSubscriptionCancel(int id) async {
    _notifications.remove(id);
    _requests.remove(id);

    while (true) {
      final state = _state;
      switch (state) {
        case _Transitioning():
          await state.future;
        case _Connected():
          await invoke(RequestCancel(id: MessageId(id)));
          return;
        case _Closed():
        case _Disconnected():
          return;
      }
    }
  }
}

_minDuration(Duration a, Duration b) => (a < b) ? a : b;

Future<(int, List<int>)> _readLocalEndpoint(String path) async {
  final file = File(path);
  final content = await file.readAsString();
  final raw = json.decode(content) as Map<String, Object?>;

  final port = raw['port'] as int;
  final authKey = HEX.decode(raw['auth_key'] as String);

  return (port, authKey);
}

class _Connector {
  final int port;
  final List<int> authKey;
  final Duration minDelay;
  final Duration maxDelay;

  const _Connector({
    required this.port,
    required this.authKey,
    this.minDelay = const Duration(milliseconds: 50),
    this.maxDelay = const Duration(seconds: 1),
  });

  Future<(Socket, Stream<Uint8List>)> connect() async {
    var delay = minDelay;
    var attempt = 0;

    while (true) {
      attempt += 1;
      try {
        final socket = await Socket.connect(
          InternetAddress.loopbackIPv4,
          port,
        ).timeout(const Duration(seconds: 5));
        final stream = socket.asBroadcastStream();

        await _authenticate(stream, socket);

        // ignore: avoid_print
        print(
          'ouisync.Client: connected to 127.0.0.1:$port on attempt $attempt',
        );
        return (socket, stream);
      } on SocketException catch (e) {
        // ignore: avoid_print
        print(
          'ouisync.Client: connect attempt $attempt to 127.0.0.1:$port '
          'failed: $e, retrying in $delay',
        );
        delay = _minDuration(delay * 2, maxDelay);
        await Future.delayed(delay);
      } catch (e) {
        // Anything other than SocketException (e.g. a timeout, or `_authenticate` failing for
        // some non-socket reason) was previously left uncaught here, silently breaking the
        // retry loop instead of retrying like every other connection failure does.
        // ignore: avoid_print
        print(
          'ouisync.Client: connect attempt $attempt to 127.0.0.1:$port '
          'failed with unexpected error: $e, retrying in $delay',
        );
        delay = _minDuration(delay * 2, maxDelay);
        await Future.delayed(delay);
      }
    }
  }

  Future<void> _authenticate(
    Stream<Uint8List> stream,
    IOSink sink,
  ) async {
    const challengeSize = 256;
    const proofSize = 32;

    final random = Random.secure();
    final hmac = Hmac(sha256, authKey);
    final reader = _Reader(stream);

    final clientChallenge = List<int>.generate(
      challengeSize,
      (_) => random.nextInt(256),
    );

    sink.add(clientChallenge);

    final serverProof = Digest(await reader.read(proofSize));
    if (serverProof != hmac.convert(clientChallenge)) {
      throw PermissionDenied('server authentication failed');
    }

    final serverChallenge = await reader.read(challengeSize);
    final clientProof = hmac.convert(serverChallenge);

    sink.add(clientProof.bytes);
  }
}

sealed class _SocketState {
  const _SocketState();
}

class _Transitioning extends _SocketState {
  final Future<void> future;

  const _Transitioning(this.future);
}

class _Disconnected extends _SocketState {
  const _Disconnected();
}

class _Connected extends _SocketState {
  final Socket socket;
  final StreamSubscription<Uint8List> subscription;

  const _Connected(this.socket, this.subscription);
}

class _Closed extends _SocketState {
  const _Closed();
}

class _Reader {
  final Stream<Uint8List> stream;
  final builder = BytesBuilder();

  _Reader(this.stream);

  Future<List<int>> read(int length) async {
    if (builder.length < length) {
      await for (final chunk in stream) {
        builder.add(chunk);

        if (builder.length >= length) {
          break;
        }
      }
    }

    final bytes = builder.takeBytes();
    final output = bytes.sublist(0, length);
    builder.add(bytes.sublist(length));

    return output;
  }
}

extension<K, V> on Map<K, V> {
  List<V> takeValues() {
    final values = this.values.toList();
    clear();
    return values;
  }
}
