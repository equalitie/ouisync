import 'dart:async';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'bindings.dart';
import 'exception.dart';

/// Handle to start and stop Ouisync service inside this process.
class Server {
  Pointer<Void> _handle;

  Server._(this._handle);

  /// Starts the server and bind it to the specified local socket. After this function completes the
  /// server is ready to accept client connections.
  static Future<Server> start({
    required String socketPath,
    required String configPath,
    required String storePath,
    String? debugLabel,
  }) async {
    final socketPathPtr = socketPath.toNativeUtf8(allocator: malloc);
    final configPathPtr = configPath.toNativeUtf8(allocator: malloc);
    final storePathPtr = storePath.toNativeUtf8(allocator: malloc);
    final debugLabelPtr = debugLabel != null
        ? debugLabel.toNativeUtf8(allocator: malloc)
        : nullptr;

    final completer = Completer<int>();
    final callback = NativeCallable<Callback>.listener(
      (Pointer<Void> context, int errorCode) => completer.complete(errorCode),
    );

    try {
      final handle = Bindings.instance.start(
        socketPathPtr.cast(),
        configPathPtr.cast(),
        storePathPtr.cast(),
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

      malloc.free(storePathPtr);
      malloc.free(configPathPtr);
      malloc.free(socketPathPtr);
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
    final callback = NativeCallable<Callback>.listener(
      (Pointer<Void> context, int errorCode) => completer.complete(errorCode),
    );

    try {
      Bindings.instance.stop(
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

void logInit({String? file, String tag = ''}) {
  final filePtr = file != null ? file.toNativeUtf8(allocator: malloc) : nullptr;
  final tagPtr = tag.toNativeUtf8(allocator: malloc);

  try {
    Bindings.instance.logInit(
      filePtr.cast(),
      tagPtr.cast(),
    );
  } finally {
    if (filePtr != nullptr) {
      malloc.free(filePtr);
    }

    malloc.free(tagPtr);
  }
}

/// Print log message
void logPrint(LogLevel level, String scope, String message) {
  final scopePtr = scope.toNativeUtf8(allocator: malloc);
  final messagePtr = message.toNativeUtf8(allocator: malloc);

  try {
    Bindings.instance.logPrint(
      level.encode(),
      scopePtr.cast(),
      messagePtr.cast(),
    );
  } finally {
    malloc.free(messagePtr);
    malloc.free(scopePtr);
  }
}
