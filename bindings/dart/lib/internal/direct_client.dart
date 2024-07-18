import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:ffi/ffi.dart';
import 'package:msgpack_dart/msgpack_dart.dart';

import '../client.dart';
import '../bindings.dart';
import '../ouisync.dart' show Error;

/// Client to interface with ouisync running in the same process as the FlutterEngine.
/// Ouisync backend (Rust) function invokations are done through FFI.
class DirectClient extends Client {
  int _handle;
  final Stream<Uint8List> _stream;
  var _nextMessageId = 0;
  final _responses = HashMap<int, Completer<Object?>>();
  final _subscriptions = Subscriptions();

  DirectClient(this._handle, ReceivePort port) : _stream = port.cast<Uint8List>(), super() {
    unawaited(_receive());
  }

  int get handle => _handle;

  @override
  Future<T> invoke<T>(String method, [Object? args]) async {
    final id = _nextMessageId++;
    final completer = Completer();

    _responses[id] = completer;

    final request = {method: args};

    // DEBUG
    //print('send: id: $id, request: $request');

    try {
      // Message format:
      //
      // +-------------------------------------+-------------------------------------------+
      // | id (big endian 64 bit unsigned int) | request (messagepack encoded byte string) |
      // +-------------------------------------+-------------------------------------------+
      //
      // This allows the server to decode the id even if the request is malformed so it can send
      // error response back.
      final message = (BytesBuilder()
            ..add((ByteData(8)..setUint64(0, id)).buffer.asUint8List())
            ..add(serialize(request)))
          .takeBytes();

      _send(message);

      return await completer.future as T;
    } finally {
      _responses.remove(id);
    }
  }

  @override
  Future<void> close() async {
    final handle = _handle;
    _handle = 0;

    if (handle == 0) {
      return;
    }

    await _invoke_native_async(
      (port) => bindings.session_close(
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

    bindings.session_close_blocking(handle);
  }

  void _send(Uint8List data) {
    if (_handle == 0) {
      throw StateError('session has been closed');
    }

    // TODO: is there a way to do this without having to allocate whole new buffer?
    var buffer = malloc<Uint8>(data.length);

    try {
      buffer.asTypedList(data.length).setAll(0, data);
      bindings.session_channel_send(_handle, buffer, data.length);
    } finally {
      malloc.free(buffer);
    }
  }

  Future<void> _receive() async {
    await for (final bytes in _stream) {
      if (bytes.length < 8) {
        continue;
      }

      final id = bytes.buffer.asByteData().getUint64(0);
      final message = deserialize(bytes.sublist(8));

      // DEBUG
      //print('recv: id: $id, message: $message');

      if (message is! Map) {
        continue;
      }

      final isSuccess = message.containsKey('success');
      final isFailure = message.containsKey('failure');
      final isNotification = message.containsKey('notification');

      if (isSuccess || isFailure) {
        final responseCompleter = _responses.remove(id);
        if (responseCompleter == null) {
          print('unsolicited response');
          continue;
        }

        if (isSuccess) {
          _handleResponseSuccess(responseCompleter, message['success']);
        } else if (isFailure) {
          _handleResponseFailure(responseCompleter, message['failure']);
        }
      } else if (isNotification) {
        _subscriptions.handle(id, message['notification']);
      } else {
        final responseCompleter = _responses.remove(id);
        if (responseCompleter != null) {
          _handleInvalidResponse(responseCompleter);
        }
      }
    }
  }

  void _handleResponseSuccess(Completer<Object?> completer, Object? payload) {
    if (payload == "none") {
      completer.complete(null);
      return;
    }

    if (payload is Map && payload.length == 1) {
      completer.complete(payload.entries.single.value);
    } else {
      _handleInvalidResponse(completer);
    }
  }

  void _handleResponseFailure(Completer<Object?> completer, Object? payload) {
    if (payload is! List) {
      _handleInvalidResponse(completer);
      return;
    }

    final code = payload[0];
    final message = payload[1];

    if (code is! int || message is! String) {
      _handleInvalidResponse(completer);
      return;
    }

    final error = Error(ErrorCode.decode(code), message);
    completer.completeError(error);
  }

  void _handleInvalidResponse(Completer<Object?> completer) {
    final error = Exception('invalid response');
    completer.completeError(error);
  }

  Future<void> copyToRawFd(int fileHandle, int fd) {
      return _invoke_native_async(
        (port) => bindings.file_copy_to_raw_fd(
          handle,
          fileHandle,
          fd,
          NativeApi.postCObject,
          port,
        ),
      );
  }

  Subscriptions subscriptions() => _subscriptions;
}

// Helper to invoke a native async function.
Future<void> _invoke_native_async(void Function(int) fun) async {
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
