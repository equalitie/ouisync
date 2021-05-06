import 'dart:async';
import 'dart:collection';
import 'dart:ffi';
import 'dart:isolate';
import 'dart:math';
import 'package:ffi/ffi.dart';
import '../gen/ouisync_bindings.dart';

Future<void> main() async {
  final session = Session(lib: DynamicLibrary.open('../target/debug/libouisync.so'));
  final repo = await Repository.open(session, '/home/adam/.local/share/ouisync/db');

  final entries = await repo.readDir('/');

  for (var entry in entries) {
    print('${entry.name}: ${entry.type}');
  }

  entries.dispose();
  repo.dispose();
  session.dispose();
}

class Session {
  final Bindings bindings;

  Session({DynamicLibrary? lib})
    : bindings = Bindings(lib ?? _defaultLib())
  {
    invokeSync<void>(
      (error) => bindings.create_session(NativeApi.postCObject.cast<Void>(), error));
  }

  void dispose() {
    bindings.destroy_session();
  }
}

class Repository {
  final Bindings bindings;
  final int handle;

  Repository._(this.bindings, this.handle);

  static Future<Repository> open(Session session, String store) async {
    final bindings = session.bindings;
    return Repository._(bindings, await invokeAsync<int>(
      (port, error) => bindings.open_repository(
        store.toNativeUtf8().cast<Int8>(),
        port,
        error)));
  }

  Future<DirEntries> readDir(String path) async {
    return DirEntries._(bindings, await invokeAsync<int>(
      (port, error) => bindings.read_dir(
        handle,
        path.toNativeUtf8().cast<Int8>(),
        port,
        error)));
  }

  void dispose() {
    bindings.close_repository(handle);
  }
}

class DirEntries with IterableMixin<DirEntry> {
  final Bindings bindings;
  final int handle;

  DirEntries._(this.bindings, this.handle);

  void dispose() {
    bindings.destroy_dir_entries(handle);
  }

  @override
  Iterator<DirEntry> get iterator => DirEntriesIterator._(bindings, handle);
}

class DirEntry {
  final String name;
  final int type;

  DirEntry._(this.name, this.type);
}

class DirEntriesIterator extends Iterator<DirEntry> {
  final Bindings bindings;
  final int handle;
  final int count;
  int index = -1;

  DirEntriesIterator._(this.bindings, this.handle)
    : count = bindings.dir_entries_count(handle);

  @override DirEntry get current {
    assert(index >= 0 && index < count);

    final name = bindings.dir_entries_name_at(handle, index);
    assert(name != nullptr);

    final type = bindings.dir_entries_type_at(handle, index);

    return DirEntry._(name.cast<Utf8>().toDartString(), type);
  }

  @override
  bool moveNext() {
    index = min(index + 1, count);
    return index < count;
  }

}

DynamicLibrary _defaultLib() {
  // TODO: this depends on the platform
  return DynamicLibrary.open('libouisync.so');
}

class ErrorHelper {
  final ptr = malloc<Pointer<Int8>>();

  ErrorHelper();

  void check() {
    if (ptr.value != nullptr) {
      final error = ptr.value.cast<Utf8>().toDartString();
      // TODO: do we need to `free` the ptr here?
      throw error;
    }
  }
}

// Helper to invoke a native async function.
Future<T> invokeAsync<T>(void Function(int, Pointer<Pointer<Int8>>) fun) async {
  final error = ErrorHelper();
  final recvPort = ReceivePort();

  fun(recvPort.sendPort.nativePort, error.ptr);

  final result = await recvPort.first;

  recvPort.close();
  error.check();

  return result;
}

// Helper to invoke native sync function.
T invokeSync<T>(T Function(Pointer<Pointer<Int8>>) fun) {
  final helper = ErrorHelper();
  final result = fun(helper.ptr);
  helper.check();
  return result;
}

