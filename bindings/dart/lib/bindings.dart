import 'dart:ffi';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path/path.dart';

export 'bindings.g.dart';

/// Callback for `service_start` and `service_stop`.
typedef StatusCallback = Void Function(Pointer<Void>, Uint16);

/// Callback for `log_init`.
typedef LogCallback = Void Function(Uint8, Pointer<Char>);

///
typedef ServiceStart = Pointer<Void> Function(
  Pointer<Char>,
  Pointer<Char>,
  Pointer<NativeFunction<StatusCallback>>,
  Pointer<Void>,
);

typedef _ServiceStartC = Pointer<Void> Function(
  Pointer<Char>,
  Pointer<Char>,
  Pointer<NativeFunction<StatusCallback>>,
  Pointer<Void>,
);

typedef ServiceStop = void Function(
    Pointer<Void>, Pointer<NativeFunction<StatusCallback>>, Pointer<Void>);

typedef _ServiceStopC = Void Function(
    Pointer<Void>, Pointer<NativeFunction<StatusCallback>>, Pointer<Void>);

typedef LogInit = int Function(
  Pointer<Char>,
  Pointer<NativeFunction<LogCallback>>,
  Pointer<Char>,
);

typedef _LogInitC = Uint16 Function(
  Pointer<Char>,
  Pointer<NativeFunction<LogCallback>>,
  Pointer<Char>,
);

class Bindings {
  Bindings(DynamicLibrary library)
      : serviceStart = library
            .lookup<NativeFunction<_ServiceStartC>>('service_start')
            .asFunction(),
        serviceStop = library
            .lookup<NativeFunction<_ServiceStopC>>('service_stop')
            .asFunction(),
        logInit =
            library.lookup<NativeFunction<_LogInitC>>('log_init').asFunction();

  /// Bidings instance that uses the default library.
  static Bindings instance = Bindings(_defaultLib());

  final ServiceStart serviceStart;
  final ServiceStop serviceStop;
  final LogInit logInit;
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
  final String path;

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
