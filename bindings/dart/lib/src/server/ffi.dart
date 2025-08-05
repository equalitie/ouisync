import 'dart:async';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'bindings.dart';
import '../server.dart';
import '../../generated/api.g.dart' show ErrorCode, OuisyncException;

class FfiServer extends Server {
  Pointer<Void> _handle = nullptr;

  FfiServer({required super.configPath, super.debugLabel});

  @override
  Future<void> initLog() async => Bindings.instance.initLog();

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
