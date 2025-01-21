import 'dart:async';
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'bindings.dart';
import 'exception.dart';

/// Handle to start and stop Ouisync service inside this process.
class Server {
  Pointer<Void> _handle;

  Server._(this._handle);

  /// Starts the server. After this function completes the server is ready to accept client
  /// connections.
  static Future<Server> start({
    required String configPath,
    String? debugLabel,
  }) async {
    final configPathPtr = configPath.toNativeUtf8(allocator: malloc);
    final debugLabelPtr = debugLabel != null
        ? debugLabel.toNativeUtf8(allocator: malloc)
        : nullptr;

    final completer = Completer<int>();
    final callback = NativeCallable<StatusCallback>.listener(
      (Pointer<Void> context, int errorCode) => completer.complete(errorCode),
    );

    try {
      final handle = Bindings.instance.startService(
        configPathPtr.cast(),
        debugLabelPtr.cast(),
        callback.nativeFunction,
        nullptr,
      );

      final errorCode = ErrorCode.decode(await completer.future);

      if (errorCode == ErrorCode.ok) {
        return Server._(handle);
      } else {
        throw OuisyncException(errorCode);
      }
    } finally {
      callback.close();

      if (debugLabelPtr != nullptr) {
        malloc.free(debugLabelPtr);
      }

      malloc.free(configPathPtr);
    }
  }

  /// Stops the server.
  Future<void> stop() async {
    final handle = _handle;
    _handle = nullptr;

    if (handle == nullptr) {
      // Already stopped.
      return;
    }

    final completer = Completer<int>();
    final callback = NativeCallable<StatusCallback>.listener(
      (Pointer<Void> context, int errorCode) => completer.complete(errorCode),
    );

    try {
      Bindings.instance.stopService(
        handle,
        callback.nativeFunction,
        nullptr,
      );

      final errorCode = ErrorCode.decode(await completer.future);

      if (errorCode != ErrorCode.ok) {
        throw OuisyncException(errorCode);
      }
    } finally {
      callback.close();
    }
  }
}

void initLog({
  String? file,
  Function(LogLevel, String)? callback,
}) {
  final filePtr = file != null ? file.toNativeUtf8(allocator: malloc) : nullptr;

  NativeCallable<LogCallback>? nativeCallback;

  if (callback != null) {
    nativeCallback = NativeCallable<LogCallback>.listener(
      (LogMessage message) {
        callback(
          LogLevel.decode(message.level),
          utf8.decode(message.ptr.asTypedList(message.len)),
        );

        Bindings.instance.releaseLogMessage(message);
      },
    );
  }

  try {
    Bindings.instance.initLog(
      filePtr.cast(),
      nativeCallback?.nativeFunction ?? nullptr,
    );
  } finally {
    if (filePtr != nullptr) {
      malloc.free(filePtr);
    }
  }
}
