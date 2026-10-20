import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';
import 'package:path/path.dart' show join;

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
  final env = Platform.environment;

  if (env.containsKey('LOGTEE_LIB')) {
    // user provided library path
    return DynamicLibrary.open(env['LOGTEE_LIB']!);
  }

  if (Platform.isMacOS && env.containsKey('FLUTTER_TEST')) {
    // guess the location of the framework built by darwin/build-framework.sh
    return DynamicLibrary.open(
      join('ouisync', 'logtee', 'dart', 'darwin', 'Logtee.framework', 'Logtee'),
    );
  }

  if (Platform.isIOS || Platform.isMacOS) {
    // In the packaged app the native core is embedded as Logtee.framework (a loose
    // .dylib is rejected by App Store validation). dyld resolves this @rpath-relative
    // path to the framework binary under the app's Frameworks dir. See
    // bindings/dart/lib/src/server/bindings.dart for the analogous OuisyncService case.
    return DynamicLibrary.open('Logtee.framework/Logtee');
  }

  // the default library name depends on the operating system
  final String name;
  if (Platform.isLinux || Platform.isAndroid) {
    name = 'liblogtee.so';
  } else if (Platform.isWindows) {
    name = 'logtee.dll';
  } else {
    throw Exception('unsupported platform ${Platform.operatingSystem}');
  }

  return DynamicLibrary.open(name);
}
