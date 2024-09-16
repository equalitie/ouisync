import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:ffi/ffi.dart';

import 'message_matcher.dart';

import '../client.dart';
import '../bindings.dart';
import '../ouisync.dart' show Error;

/// Client to interface with ouisync running in the same process as the FlutterEngine.
/// Ouisync backend (Rust) function invokations are done through FFI.
class DirectClient extends Client {
  int _handle;
  final Stream<Uint8List> _stream;
  final MessageMatcher _messageMatcher = MessageMatcher();
  final Bindings _bindings;

  DirectClient(this._handle, ReceivePort port, this._bindings) : _stream = port.cast<Uint8List>(), super() {
    unawaited(_receive());
  }

  int get handle => _handle;

  @override
  Future<T> invoke<T>(String method, [Object? args]) async {
    // NOTE: Async is used for sender because that's what the MessageMatcher
    // expects, and the MessageMatcher expects it because sending over
    // ChannelClient is async.  In fact, the rust implementation uses an
    // umbounded buffer inside the `session_channel_send`, so it might be
    // better to convert that into a bounded buffer and make the
    // `session_channel_send` async as well.
    return await _messageMatcher.sendAndAwaitResponse(method, args, (Uint8List message) async {
        _send(message);
    });
  }

  @override
  Future<void> close() async {
    final handle = _handle;
    _handle = 0;

    if (handle == 0) {
      return;
    }

    _messageMatcher.close();

    await _invokeNativeAsync(
      (port) => _bindings.session_close(
        handle,
        NativeApi.postCObject,
        port,
      ),
    );
  }

  void closeSync() {
    final handle = _handle;
    _handle = 0;

    if (handle == 0) {
      return;
    }

    _messageMatcher.close();

    _bindings.session_close_blocking(handle);
  }

  Future<void> _receive() async {
    await for (final bytes in _stream) {
      _messageMatcher.handleResponse(bytes);
    }
  }

  Future<void> copyToRawFd(int fileHandle, int fd) {
      return _invokeNativeAsync(
        (port) => _bindings.file_copy_to_raw_fd(
          handle,
          fileHandle,
          fd,
          NativeApi.postCObject,
          port,
        ),
      );
  }

  @override
  Subscriptions subscriptions() => _messageMatcher.subscriptions();

  void _send(Uint8List data) {
    // TODO: is there a way to do this without having to allocate whole new buffer?
    var buffer = malloc<Uint8>(data.length);
  
    try {
      buffer.asTypedList(data.length).setAll(0, data);
      _bindings.session_channel_send(_handle, buffer, data.length);
    } finally {
      malloc.free(buffer);
    }
  }

}

// Helper to invoke a native async function.
Future<void> _invokeNativeAsync(void Function(int) fun) async {
  final recvPort = ReceivePort();

  try {
    fun(recvPort.sendPort.nativePort);

    final bytes = await recvPort.cast<Uint8List>().first;

    if (bytes.isEmpty) {
      return;
    }

    final code = ErrorCode.decode(bytes.buffer.asByteData().getUint16(0));
    final message = utf8.decode(bytes.sublist(2));

    if (code == ErrorCode.ok) {
      return;
    } else {
      throw Error(code, message);
    }
  } finally {
    recvPort.close();
  }
}
