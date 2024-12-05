import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

import 'bindings.dart';
import 'exception.dart';

class Server {
  final Isolate isolate;
  final Future<void> terminated;

  Server._(this.isolate, this.terminated);

  static Future<Server> start({
    required String socketPath,
    required String configPath,
    required String storePath,
    String? debugLabel,
  }) async {
    final rx = ReceivePort();
    final tx = rx.sendPort;
    final isolate = await Isolate.spawn(
      _entryPoint,
      _Params(
        port: tx,
        socketPath: socketPath,
        configPath: configPath,
        storePath: storePath,
        debugLabel: debugLabel,
      ),
    );

    final terminated = rx.first.then((output) {
      final errorCode = output as ErrorCode;

      if (errorCode != ErrorCode.ok) {
        throw OuisyncException(errorCode);
      }
    });

    return Server._(isolate, terminated);
  }
}

class _Params {
  final SendPort port;
  final String socketPath;
  final String configPath;
  final String storePath;
  final String? debugLabel;

  _Params({
    required this.port,
    required this.socketPath,
    required this.configPath,
    required this.storePath,
    this.debugLabel,
  });
}

void _entryPoint(_Params params) {
  final bindings = Bindings.instance;
  final debugLabel = params.debugLabel;

  final rawErrorCode = _withPool((pool) => bindings.start(
        pool.toNativeUtf8(params.socketPath),
        pool.toNativeUtf8(params.configPath),
        pool.toNativeUtf8(params.storePath),
        debugLabel != null ? pool.toNativeUtf8(debugLabel) : nullptr,
      ));
  final errorCode = ErrorCode.decode(rawErrorCode);

  params.port.send(errorCode);
}

///// Runs Ouisync server and bind it to the specified local socket. Returns when the server
///// terminates.
//Future<void> runServer({
//  required String socketPath,
//  required String configPath,
//  required String storePath,
//  String? debugLabel,
//}) async {
//  final bindings = Bindings.instance;

//  final rawErrorCode = await Isolate.run(
//    () => _withPool(
//      (pool) => bindings.start(
//          pool.toNativeUtf8(socketPath),
//          pool.toNativeUtf8(configPath),
//          pool.toNativeUtf8(storePath),
//          debugLabel != null ? pool.toNativeUtf8(debugLabel) : nullptr),
//    ),
//  );

//  final errorCode = ErrorCode.decode(rawErrorCode);

//  if (errorCode != ErrorCode.ok) {
//    throw OuisyncException(errorCode);
//  }
//}

void logInit({String? file, String tag = ''}) =>
    _withPool((pool) => Bindings.instance.log_init(
          file != null ? pool.toNativeUtf8(file) : nullptr,
          pool.toNativeUtf8(tag),
        ));

/// Print log message
void logPrint(LogLevel level, String scope, String message) =>
    _withPool((pool) => Bindings.instance.log_print(
          level.encode(),
          pool.toNativeUtf8(scope),
          pool.toNativeUtf8(message),
        ));

// Call the sync function passing it a [_Pool] which will be released when the function returns.
T _withPool<T>(T Function(_Pool) fun) {
  final pool = _Pool();

  try {
    return fun(pool);
  } finally {
    pool.release();
  }
}

// Allocator that tracks all allocations and frees them all at the same time.
class _Pool implements Allocator {
  List<Pointer<NativeType>> ptrs = [];

  @override
  Pointer<T> allocate<T extends NativeType>(int byteCount, {int? alignment}) {
    final ptr = malloc.allocate<T>(byteCount, alignment: alignment);
    ptrs.add(ptr);
    return ptr;
  }

  @override
  void free(Pointer<NativeType> ptr) {
    // free on [release]
  }

  void release() {
    for (var ptr in ptrs) {
      malloc.free(ptr);
    }
  }

  // Convenience function to convert a dart string to a C-style nul-terminated utf-8 encoded
  // string pointer. The pointer is allocated using this pool.
  Pointer<Char> toNativeUtf8(String str) =>
      str.toNativeUtf8(allocator: this).cast<Char>();
}
