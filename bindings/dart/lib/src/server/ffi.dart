import 'dart:async';
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'bindings.dart';
import '../server.dart';
import '../../generated/api.g.dart' show ErrorCode, LogLevel, OuisyncException;

class FfiServer extends Server {
  Pointer<Void> _handle = nullptr;

  FfiServer({required super.configPath, super.debugLabel});

  @override
  void initLog({
    bool stdout = false,
    String? file,
    Function(LogLevel, String)? callback,
  }) {
    final filePtr =
        file != null ? file.toNativeUtf8(allocator: malloc) : nullptr;

    NativeCallable<LogCallback>? nativeCallback;

    if (callback != null) {
      nativeCallback = NativeCallable<LogCallback>.listener(
        (int level, Pointer<Uint8> ptr, int len, int cap) {
          callback(
            LogLevel.fromInt(level) ?? LogLevel.error,
            utf8.decode(ptr.asTypedList(len)),
          );

          Bindings.instance.releaseLogMessage(ptr, len, cap);
        },
      );
    }

    try {
      Bindings.instance.initLog(
        stdout ? 1 : 0,
        filePtr.cast(),
        nativeCallback?.nativeFunction ?? nullptr,
      );
    } finally {
      if (filePtr != nullptr) {
        malloc.free(filePtr);
      }
    }
  }

  @override
  Future<void> start() async {
    if (_handle != nullptr) {
      return;
    }

    final configPathPtr = configPath.toNativeUtf8(allocator: malloc);
    final debugLabelPtr =
        debugLabel?.toNativeUtf8(allocator: malloc) ?? nullptr;

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

      final errorCode =
          ErrorCode.fromInt(await completer.future) ?? ErrorCode.other;

      if (errorCode == ErrorCode.ok) {
        _handle = handle;
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

  @override
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

      final errorCode =
          ErrorCode.fromInt(await completer.future) ?? ErrorCode.other;

      if (errorCode != ErrorCode.ok) {
        throw OuisyncException(errorCode);
      }
    } finally {
      callback.close();
    }
  }
}
