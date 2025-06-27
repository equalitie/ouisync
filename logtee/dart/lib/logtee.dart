import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

class Logtee {
  Pointer<Void> _handle = nullptr;

  Logtee._(this._handle);

  /// Start capturing log messages to the file at `path`.
  static Logtee start(
    String path, {
    int maxFileSize = 1024 * 1024,
    int maxFiles = 1,
  }) {
    final pathPtr = path.toNativeUtf8(allocator: malloc);

    try {
      final handle = _bindings.start(pathPtr.cast(), maxFileSize, maxFiles);
      return Logtee._(handle);
    } finally {
      malloc.free(pathPtr);
    }
  }

  /// Stop capturing log messages.
  void stop() {
    if (_handle == nullptr) return;

    _bindings.stop(_handle);
    _handle = nullptr;
  }
}

typedef Start = Pointer<Void> Function(Pointer<Char>, int, int);
typedef _StartC = Pointer<Void> Function(Pointer<Char>, Uint64, Uint32);

typedef Stop = void Function(Pointer<Void>);
typedef _StopC = Void Function(Pointer<Void>);

final _Bindings _bindings = _Bindings(_defaultLib());

class _Bindings {
  _Bindings(DynamicLibrary library)
    : start = library.lookupFunction<_StartC, Start>('logtee_start'),
      stop = library.lookupFunction<_StopC, Stop>('logtee_stop');

  final Start start;
  final Stop stop;
}

DynamicLibrary _defaultLib() {
  // the default library name depends on the operating system
  String name() {
    final base = 'logtee';

    if (Platform.isLinux || Platform.isAndroid) {
      return 'lib$base.so';
    } else if (Platform.isWindows) {
      return '$base.dll';
    } else if (Platform.isIOS || Platform.isMacOS) {
      return 'lib$base.dylib';
    } else {
      throw Exception('unsupported platform ${Platform.operatingSystem}');
    }
  }

  return DynamicLibrary.open(name());
}
