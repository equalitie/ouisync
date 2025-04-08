import 'dart:typed_data';

import 'package:messagepack/messagepack.dart';

import '../bindings.dart' show ErrorCode, OuisyncException, Request, Response;

Uint8List encodeMessage(int id, Request message) {
  // Message format:
  //
  // +-------------------------------------+-------------------------------------------+
  // | id (big endian 64 bit unsigned int) | request (messagepack encoded byte string) |
  // +-------------------------------------+-------------------------------------------+
  //
  // This allows the server to decode the id even if the request is malformed so it can send
  // error response back.

  final p = Packer();
  message.encode(p);

  return (BytesBuilder()
        ..add((ByteData(8)..setUint64(0, id, Endian.big)).buffer.asUint8List())
        ..add(p.takeBytes()))
      .takeBytes();
}

DecodeResult decodeMessage(Uint8List bytes) {
  if (bytes.length < 8) {
    return MalformedMessage._();
  }

  final id = bytes.buffer.asByteData().getUint64(0, Endian.big);
  final u = Unpacker(bytes.sublist(8));

  try {
    if (u.unpackMapLength() != 1) {
      return MalformedPayload._(id);
    }

    final key = u.unpackString();
    if (key == null) {
      return MalformedPayload._(id);
    }

    switch (key) {
      case 'Success':
        final response = Response.decode(u);
        if (response == null) {
          return MalformedPayload._(id);
        }

        return MessageSuccess._(id, response);
      case 'Failure':
        if (u.unpackListLength() != 3) {
          return MalformedPayload._(id);
        }

        final code = ErrorCode.decode(u);
        if (code == null) {
          return MalformedPayload._(id);
        }

        final message = u.unpackString();
        if (message == null) {
          return MalformedPayload._(id);
        }

        final numSources = u.unpackListLength();
        final sources = Iterable.generate(numSources, (_) => u.unpackString())
            .whereType<String>()
            .toList();

        final error = OuisyncException(code, message, sources);
        return MessageFailure._(id, error);
      default:
        return MalformedPayload._(id);
    }
  } on FormatException {
    return MalformedPayload._(id);
  }
}

sealed class DecodeResult {}

class MessageSuccess extends DecodeResult {
  final int id;
  final Response payload;

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
