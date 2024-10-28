// ignore_for_file: camel_case_types
// ignore_for_file: non_constant_identifier_names

import 'dart:ffi';
import 'dart:io';

import 'package:path/path.dart';
import 'package:flutter/foundation.dart' show kReleaseMode;

export 'bindings.g.dart';

Bindings? bindings;

typedef PostCObject = Int8 Function(Int64, Pointer<Dart_CObject>);

typedef _session_create_c = SessionCreateResult Function(
  Uint8,
  Pointer<Char>,
  Pointer<Char>,
  Pointer<Char>,
  Pointer<NativeFunction<PostCObject>>,
  Int64,
);
typedef session_create_dart = SessionCreateResult Function(
  int,
  Pointer<Char>,
  Pointer<Char>,
  Pointer<Char>,
  Pointer<NativeFunction<PostCObject>>,
  int,
);

typedef _session_channel_send_c = Void Function(Uint64, Pointer<Uint8>, Uint64);
typedef session_channel_send_dart = void Function(int, Pointer<Uint8>, int);

typedef _session_close_c = Void Function(
    Uint64, Pointer<NativeFunction<PostCObject>>, Int64);
typedef session_close_dart = void Function(
    int, Pointer<NativeFunction<PostCObject>>, int);

typedef _session_close_blocking_c = Void Function(Uint64);
typedef session_close_blocking_dart = void Function(int);

typedef _file_copy_to_raw_fd_c = Void Function(
    Uint64, Uint64, Int, Pointer<NativeFunction<PostCObject>>, Int64);
typedef file_copy_to_raw_fd_dart = void Function(
    int, int, int, Pointer<NativeFunction<PostCObject>>, int);

typedef _log_print_c = Void Function(Uint8, Pointer<Char>, Pointer<Char>);
typedef log_print_dart = void Function(int, Pointer<Char>, Pointer<Char>);

typedef _free_string_c = Void Function(Pointer<Char>);
typedef free_string_dart = void Function(Pointer<Char>);

final class SessionCreateResult extends Struct {
  @Uint64()
  external int session;

  @Uint16()
  external int error_code;

  external Pointer<Char> error_message;
}

class Bindings {
  Bindings(DynamicLibrary library)
      : session_create = library
            .lookup<NativeFunction<_session_create_c>>('session_create_dart')
            .asFunction(),
        session_channel_send = library
            .lookup<NativeFunction<_session_channel_send_c>>(
                'session_channel_send')
            .asFunction(),
        session_close = library
            .lookup<NativeFunction<_session_close_c>>('session_close_dart')
            .asFunction(),
        session_close_blocking = library
            .lookup<NativeFunction<_session_close_blocking_c>>(
                'session_close_blocking')
            .asFunction(),
        file_copy_to_raw_fd = library
            .lookup<NativeFunction<_file_copy_to_raw_fd_c>>(
                'file_copy_to_raw_fd_dart')
            .asFunction(),
        log_print = library
            .lookup<NativeFunction<_log_print_c>>('log_print')
            .asFunction(),
        free_string = library
            .lookup<NativeFunction<_free_string_c>>('free_string')
            .asFunction();

  static Bindings loadDefault() {
    return Bindings(_defaultLib());
  }

  final session_create_dart session_create;
  final session_channel_send_dart session_channel_send;
  final session_close_dart session_close;
  final session_close_blocking_dart session_close_blocking;
  final file_copy_to_raw_fd_dart file_copy_to_raw_fd;
  final log_print_dart log_print;
  final free_string_dart free_string;
}

DynamicLibrary _defaultLib() {
  final env = Platform.environment;

  // the default library name depends on the operating system
  late final String name;
  final base = 'ouisync_ffi';
  if (Platform.isLinux || Platform.isAndroid) {
    name = 'lib$base.so';
  } else if (Platform.isWindows) {
    name = '$base.dll';
  } else if (Platform.isIOS || Platform.isMacOS) {
    name = 'lib$base.dylib' ;
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
