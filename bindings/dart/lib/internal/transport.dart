import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

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

class NamedPipeTransport extends Transport {
  final RandomAccessFile _file;
  bool _open = true;

  NamedPipeTransport._(this._file);

  static Future<NamedPipeTransport> connect(String path) async =>
      NamedPipeTransport._(await File(path).open(mode: FileMode.write));

  @override
  Stream<Uint8List> get stream async* {
    while (_open) {
      yield await _file.read(4096);
    }
  }

  @override
  IOSink get sink => IOSink(_NamedPipeConsumer(_file));

  @override
  Future<void> close() async {
    _open = false;
    await _file.close();
  }
}

class _NamedPipeConsumer implements StreamConsumer<List<int>> {
  final RandomAccessFile file;

  _NamedPipeConsumer(this.file);

  @override
  Future<void> addStream(Stream<List<int>> stream) async {
    await for (final chunk in stream) {
      await file.writeFrom(chunk);
    }
  }

  @override
  Future<void> close() => file.close();
}
