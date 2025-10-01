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
        RequestSessionUnsubscribe;
import 'length_delimited_codec.dart';
import 'message_codec.dart';

class Client {
  final _Connector _connector;

  _SocketState _state = const _Disconnected();

  var _nextMessageId = 0;
  final _responses = <int, Completer<Object?>>{};
  final _notifications = <int, StreamSink<Object?>>{};

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

        final sink = _notifications[message.id];
        if (sink != null) {
          sink.add(message.payload);
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

    for (final sink in _notifications.takeValues()) {
      sink.addError(error, st);
    }
  }

  void _receiveDone() {
    for (final completer in _responses.takeValues()) {
      completer.completeError(SocketException.closed());
    }

    for (final sink in _notifications.takeValues()) {
      unawaited(sink.close());
    }

    _state = _Disconnected();
  }

  Future<void> _onSubscriptionListen(
    int id,
    Request request,
    StreamSink<Object?> sink,
  ) async {
    _notifications[id] = sink;

    try {
      await _invokeWithMessageId(id, request);
    } catch (e, st) {
      _notifications.remove(id);
      sink.addError(e, st);
    }
  }

  Future<void> _onSubscriptionCancel(int id) async {
    _notifications.remove(id);

    while (true) {
      final state = _state;
      switch (state) {
        case _Transitioning():
          await state.future;
        case _Connected():
          await invoke(RequestSessionUnsubscribe(id: MessageId(id)));
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

    while (true) {
      try {
        final socket = await Socket.connect(InternetAddress.loopbackIPv4, port);
        final stream = socket.asBroadcastStream();

        await _authenticate(stream, socket);

        return (socket, stream);
      } on SocketException {
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
