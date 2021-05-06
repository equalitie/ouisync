import 'dart:async';
import 'dart:ffi';
import 'dart:isolate';
import 'package:ffi/ffi.dart';
import '../gen/ouisync_bindings.dart';

Future<void> main() async {
  final lib = DynamicLibrary.open('../target/debug/libouisync.so');

  final store_dart_post_cobject = lib.lookupFunction<store_dart_post_cobject_c,
      store_dart_post_cobject_dart>('store_dart_post_cobject');
  store_dart_post_cobject(NativeApi.postCObject);

  final bindings = OuisyncBindings(lib);

  final repo = Repository(bindings, '~/.local/share/ouisync/db');
  await repo.readDir('/');
  repo.dispose();
}

class Repository {
  final OuisyncBindings bindings;
  final Pointer<Session> handle;

  Repository(this.bindings, String db_path)
      : handle = bindings.open_repository(db_path.toNativeUtf8().cast<Int8>());

  Future<void> readDir(String path) async {
    final pathPointer = path.toNativeUtf8();
    final receivePort = ReceivePort();

    bindings.read_directory(
        receivePort.sendPort.nativePort, handle, pathPointer.cast<Int8>());

    final result = await receivePort.first;

    if (result == 0) {
      throw 'error';
    }
  }

  void dispose() {
    bindings.close_repository(handle);
  }
}

typedef store_dart_post_cobject_c = Void Function(
  Pointer<NativeFunction<Int8 Function(Int64, Pointer<Dart_CObject>)>> ptr,
);
typedef store_dart_post_cobject_dart = void Function(
  Pointer<NativeFunction<Int8 Function(Int64, Pointer<Dart_CObject>)>> ptr,
);
