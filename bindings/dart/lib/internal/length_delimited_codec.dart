import 'dart:async';
import 'dart:typed_data';

/// Stream transformer for length-delimited encoding. Useful for reading length-delimited frames
/// from TCP sockets, unix domain sockets or files.
///
/// The lengths should be encoded as big-endian 32 bit unsigned int.
class LengthDelimitedCodec extends StreamTransformerBase<Uint8List, Uint8List> {
  @override
  Stream<Uint8List> bind(Stream<Uint8List> stream) => Stream.eventTransformed(
        stream,
        (EventSink<Uint8List> sink) => _DecodeSink(sink),
      );
}

/// Encode message into length-delimited frame
Uint8List encodeLengthDelimited(Uint8List message) => (BytesBuilder()
      ..add(Uint8List(4)
        ..buffer.asByteData().setUint32(0, message.length, Endian.big))
      ..add(message))
    .takeBytes();

class _DecodeSink implements EventSink<Uint8List> {
  final EventSink<Uint8List> _outputSink;
  final _builder = BytesBuilder();
  int? _length;

  _DecodeSink(this._outputSink);

  @override
  add(Uint8List chunk) {
    _builder.add(chunk);

    while (true) {
      _length ??= _take(4)?.buffer.asByteData().getUint32(0, Endian.big);
      final length = _length;
      if (length == null) {
        break;
      }

      final message = _take(length);
      if (message == null) {
        break;
      } else {
        _length = null;
        _outputSink.add(message);
      }
    }
  }

  @override
  void addError(Object error, [StackTrace? st]) =>
      _outputSink.addError(error, st);

  @override
  void close() => _outputSink.close();

  Uint8List? _take(int length) {
    if (_builder.length >= length) {
      final bytes = _builder.takeBytes();
      final output = bytes.sublist(0, length);
      _builder.add(bytes.sublist(length));
      return output;
    } else {
      return null;
    }
  }
}
