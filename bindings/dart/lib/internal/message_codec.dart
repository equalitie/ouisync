import 'dart:typed_data';

import 'package:msgpack_dart/msgpack_dart.dart';

import '../exception.dart';

Uint8List encodeMessage(int id, String method, Object? args) {
  //print('send: id=$id method=$method args=$args');

  // Message format:
  //
  // +-------------------------------------+-------------------------------------------+
  // | id (big endian 64 bit unsigned int) | request (messagepack encoded byte string) |
  // +-------------------------------------+-------------------------------------------+
  //
  // This allows the server to decode the id even if the request is malformed so it can send
  // error response back.
  return (BytesBuilder()
        ..add((ByteData(8)..setUint64(0, id, Endian.big)).buffer.asUint8List())
        ..add(serialize({method: args})))
      .takeBytes();
}

DecodeResult decodeMessage(Uint8List bytes) {
  if (bytes.length < 8) {
    return MalformedMessage._();
  }

  final id = bytes.buffer.asByteData().getUint64(0, Endian.big);
  final payload = deserialize(bytes.sublist(8));

  //print('recv: id=$id, payload=$payload');

  if (payload is! Map) {
    return MalformedPayload._(id);
  }

  if (payload.containsKey('success')) {
    final success = payload.remove('success');

    if (success == "none") {
      return MessageSuccess._(id, null);
    }

    if (success is Map && success.length == 1) {
      return MessageSuccess._(id, success.entries.single.value);
    } else {
      return MalformedPayload._(id);
    }
  }

  final rawError = payload.remove('failure');
  if (rawError == null) {
    return MalformedPayload._(id);
  }

  final code = rawError[0];
  final message = rawError[1];

  if (code is! int || message is! String) {
    return MalformedPayload._(id);
  }

  final error = OuisyncException(ErrorCode.decode(code), message);
  return MessageFailure._(id, error);
}

sealed class DecodeResult {}

class MessageSuccess extends DecodeResult {
  final int id;
  final Object? payload;

  MessageSuccess._(this.id, this.payload);

  @override
  String toString() => '$runtimeType($id, $payload)';
}

class MessageFailure extends DecodeResult {
  final int id;
  final OuisyncException exception;

  MessageFailure._(this.id, this.exception);

  @override
  String toString() => '$runtimeType($id, $exception)';
}

class MalformedMessage extends DecodeResult {
  MalformedMessage._();

  @override
  String toString() => '$runtimeType';
}

class MalformedPayload extends DecodeResult {
  final int id;

  MalformedPayload._(this.id);

  @override
  String toString() => '$runtimeType($id)';
}
