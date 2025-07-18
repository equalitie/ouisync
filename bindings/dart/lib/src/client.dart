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
  final Socket _socket;
  final Stream<Uint8List> _stream;
  StreamSubscription<Uint8List>? _streamSubscription;
  int _nextMessageId = 0;
  final _responses = <int, Completer<Object?>>{};
  final _notifications = <int, StreamSink<Object?>>{};

  Client._(this._socket, this._stream) {
    _streamSubscription =
        _stream.transform(LengthDelimitedCodec()).listen(_receive);
  }

  static Future<Client> connect({
    required String configPath,
    Duration? timeout,
    Duration minWait = const Duration(milliseconds: 50),
    Duration maxWait = const Duration(seconds: 1),
  }) async {
    DateTime start = DateTime.now();
    Duration wait = minWait;
    Exception? lastException;

    final portFile = File('$configPath/local_control_port.conf');
    final authKeyFile = File('$configPath/local_control_auth_key.conf');

    final port = json.decode(await portFile.readAsString()) as int;
    final authKey = HEX.decode(json.decode(await authKeyFile.readAsString()));

    while (true) {
      if (timeout != null) {
        if (DateTime.now().difference(start) >= timeout) {
          throw lastException ?? TimeoutException('connect timeout');
        }
      }

      try {
        final socket = await Socket.connect(InternetAddress.loopbackIPv4, port);
        final stream = socket.asBroadcastStream();
        await _authenticate(stream, socket, authKey);

        return Client._(socket, stream);
      } on SocketException catch (e) {
        lastException = e;
        wait = _minDuration(wait * 2, maxWait);
        await Future.delayed(wait);
      }
    }
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
    await _streamSubscription?.cancel();
    await _socket.close();
  }

  Future<Response> _invokeWithMessageId(int id, Request request) async {
    final completer = Completer();
    final response = completer.future;

    _responses[id] = completer;

    final bytes = encodeLengthDelimited(encodeMessage(id, request));
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
    await invoke(RequestSessionUnsubscribe(id: MessageId(id)));
  }
}

_minDuration(Duration a, Duration b) => (a < b) ? a : b;

Future<void> _authenticate(
  Stream<Uint8List> stream,
  IOSink sink,
  List<int> authKey,
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
