import 'dart:ffi';
import 'dart:io';

import 'package:path/path.dart';
import 'package:flutter/foundation.dart' show kReleaseMode;

export 'bindings.g.dart';

/// Callback for `start` and `stop`.
typedef Callback = Void Function(Pointer<Void>, Uint16);

///
typedef Start = Pointer<Void> Function(
  Pointer<Char>,
  Pointer<Char>,
  Pointer<Char>,
  Pointer<Char>,
  Pointer<NativeFunction<Callback>>,
  Pointer<Void>,
);

typedef _StartC = Pointer<Void> Function(
  Pointer<Char>,
  Pointer<Char>,
  Pointer<Char>,
  Pointer<Char>,
  Pointer<NativeFunction<Callback>>,
  Pointer<Void>,
);

typedef Stop = void Function(
    Pointer<Void>, Pointer<NativeFunction<Callback>>, Pointer<Void>);

typedef _StopC = Void Function(
    Pointer<Void>, Pointer<NativeFunction<Callback>>, Pointer<Void>);

typedef LogInit = int Function(Pointer<Char>, Pointer<Char>);

typedef _LogInitC = Uint16 Function(
  Pointer<Char>,
  Pointer<Char>,
);

typedef LogPrint = void Function(int, Pointer<Char>, Pointer<Char>);

typedef _LogPrintC = Void Function(
  Uint8,
  Pointer<Char>,
  Pointer<Char>,
);

class Bindings {
  Bindings(DynamicLibrary library)
      : start = library
            .lookup<NativeFunction<_StartC>>('ouisync_start')
            .asFunction(),
        stop =
            library.lookup<NativeFunction<_StopC>>('ouisync_stop').asFunction(),
        logInit = library
            .lookup<NativeFunction<_LogInitC>>('ouisync_log_init')
            .asFunction(),
        logPrint = library
            .lookup<NativeFunction<_LogPrintC>>('ouisync_log_print')
            .asFunction();

  /// Bidings instance that uses the default library.
  static Bindings instance = Bindings(_defaultLib());

  final Start start;
  final Stop stop;
  final LogInit logInit;
  final LogPrint logPrint;
}

DynamicLibrary _defaultLib() {
  final env = Platform.environment;

  // the default library name depends on the operating system
  late final String name;
  final base = 'ouisync_service';
  if (Platform.isLinux || Platform.isAndroid) {
    name = 'lib$base.so';
  } else if (Platform.isWindows) {
    name = '$base.dll';
  } else if (Platform.isIOS || Platform.isMacOS) {
    name = 'lib$base.dylib';
  } else {
    throw Exception('unsupported platform ${Platform.operatingSystem}');
  }

  // full path to loadable library
  late final String path;
  if (env.containsKey('OUISYNC_LIB')) {
    // user provided library path
    path = env['OUISYNC_LIB']!;
  } else if (env.containsKey('FLUTTER_TEST')) {
    // guess the location of flutter's build output
    final String build;
    if (Platform.isMacOS) {
      build = join(dirname(Platform.script.toFilePath()), 'ouisync');
    } else {
      build = join('..', '..');
    }
    path = join(build, 'target', kReleaseMode ? 'release' : 'debug', name);
  } else {
    // assume that the library is available globally by name only
    path = name;
  }

  if (Platform.isIOS) {
    // TODO: something about this?!
    return DynamicLibrary.process();
  } else {
    return DynamicLibrary.open(path);
  }
}
