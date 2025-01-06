import 'dart:async';
import 'dart:ffi';
import 'dart:io';
import 'dart:typed_data';
import 'package:ffi/ffi.dart';
import 'windows/async.dart';

abstract class Transport {
  Stream<Uint8List> get stream;
  IOSink get sink;

  Future<void> close();

  static Future<Transport> connect(String path) {
    if (Platform.isLinux ||
        Platform.isMacOS ||
        Platform.isAndroid ||
        Platform.isIOS) {
      return UnixSocketTransport.connect(path);
    }

    if (Platform.isWindows) {
      return NamedPipeTransport.connect(path);
    }

    throw UnsupportedError(
        'platform not supported: ${Platform.operatingSystem}');
  }
}

class UnixSocketTransport extends Transport {
  final Socket _socket;

  UnixSocketTransport._(this._socket);

  static Future<UnixSocketTransport> connect(String path) =>
      Socket.connect(InternetAddress(path, type: InternetAddressType.unix), 0)
          .then((socket) => UnixSocketTransport._(socket));

  @override
  Stream<Uint8List> get stream => _socket;

  @override
  IOSink get sink => _socket;

  @override
  Future<void> close() => _socket.close();
}

/* Ever since [this](https://github.com/dart-lang/sdk/commit/f9c603e76b22d317f0)
very "advantageous" commit was merged, dart can no longer open named pipes on
windows. Which is... fine because it strongly believes that all files must be
[half duplex](https://api.dart.dev/dart-io/RandomAccessFile-class.html) so it
wouldn't have worked for our use case anyway.

Thus we have to open the pipes directly, but we can't just use blocking i/o or
we'd be back to the same half-duplex problem. We also can't implicitly use
the builtin dart event loop for this purpose because there's no API for it.

Currently, we have two implementations that work around this, the better of
which cannot be used due to a bug (see windows/iocp.dart for details). */
class NamedPipeTransport extends Transport implements StreamConsumer<List<int>> {
  final NamedPipe _file;

  NamedPipeTransport._(this._file);

  static Future<NamedPipeTransport> connect(String path) async =>
      NamedPipeTransport._(await NamedPipe.open(path));

  @override
  Stream<Uint8List> get stream async* {
    final size = 4096;
    final buffer = calloc<Uint8>(size);
    final view = buffer.asTypedList(size);
    try {
      while (true) { // closing while concurrently reading is an error
        final count = await _file.read(buffer, size);
        if (count == 0) { break; } // reading 0 bytes is graceful eof
        yield view.sublist(0, count);
      }
    } finally {
      calloc.free(buffer);
    }
  }

  @override
  IOSink get sink => IOSink(this);

  @override
  Future<void> close() async {
    _file.close();
  }
  
  @override
  Future<void> addStream(Stream<List<int>> stream) async {
    await for (final chunk in stream) {
      final buffer = calloc<Uint8>(chunk.length);
      buffer.asTypedList(chunk.length).setAll(0, chunk);
      try {
        int offset = 0;
        while (offset < chunk.length) {
          offset += await _file.write(buffer + offset, chunk.length - offset);
        }
      } finally {
        calloc.free(buffer);
      }
    }
  }
}
