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
  @override
  final Stream<Uint8List> stream;

  @override
  final IOSink sink;

  NamedPipeTransport._(this.stream, this.sink);

  static Future<NamedPipeTransport> connect(String path) {
    final file = File(path);
    final stream = file.openRead().cast<Uint8List>();
    final sink = file.openWrite(mode: FileMode.append);

    return Future.value(NamedPipeTransport._(stream, sink));
  }

  @override
  Future<void> close() => sink.close();
}
