import 'dart:async';
import 'dart:collection';
import 'dart:ffi';
import 'dart:isolate';
import 'dart:math';
import 'package:ffi/ffi.dart';
import '../gen/ouisync_bindings.dart';

Future<void> main() async {
  final session = await Session.create(
    '/home/adam/.local/share/ouisync/db',
    lib: DynamicLibrary.open('../target/debug/libouisync.so')
  );

  final repo = await Repository.open(session);
  final entries = await repo.readDir('/');

  for (var entry in entries) {
    print('${entry.name}: ${entry.type}');
  }

  await repo.createDir('stuffz/thingies');

  entries.dispose();
  repo.dispose();
  session.dispose();
}

class Session {
  final Bindings bindings;

  Session._(this.bindings);

  static Future<Session> create(String store, {DynamicLibrary? lib}) async {
    final bindings = Bindings(lib ?? _defaultLib());

    await invokeAsync<void>(
      (port, error) => bindings.session_create(
        NativeApi.postCObject.cast<Void>(),
        store.toNativeUtf8().cast<Int8>(),
        port,
        error));

    return Session._(bindings);
  }

  void dispose() {
    bindings.session_destroy();
  }
}

class Repository {
  final Bindings bindings;
  final int handle;

  Repository._(this.bindings, this.handle);

  static Future<Repository> open(Session session) async {
    final bindings = session.bindings;
    return Repository._(bindings, await invokeAsync<int>(
      (port, error) => bindings.repository_open(port, error)));
  }

  Future<DirEntries> readDir(String path) async =>
    DirEntries._(bindings, await invokeAsync<int>(
      (port, error) => bindings.repository_read_dir(
        handle,
        path.toNativeUtf8().cast<Int8>(),
        port,
        error)));

  Future<void> createDir(String path) =>
    invokeAsync<void>(
      (port, error) => bindings.repository_create_dir(
        handle,
        path.toNativeUtf8().cast<Int8>(),
        port,
        error
      ));


  void dispose() {
    bindings.repository_close(handle);
  }
}

enum DirEntryType {
  File,
  Directory,
}

class DirEntry {
  final Bindings bindings;
  final int handle;

  DirEntry._(this.bindings, this.handle);

  String get name => bindings.dir_entry_name(handle).cast<Utf8>().toDartString();

  DirEntryType get type {
    switch (bindings.dir_entry_type(handle)) {
      case DIR_ENTRY_FILE:
        return DirEntryType.File;
      case DIR_ENTRY_DIRECTORY:
        return DirEntryType.Directory;
      default:
        throw Error("invalid dir entry type");
    }
  }
}

class DirEntries with IterableMixin<DirEntry> {
  final Bindings bindings;
  final int handle;

  DirEntries._(this.bindings, this.handle);

  void dispose() {
    bindings.dir_entries_destroy(handle);
  }

  @override
  Iterator<DirEntry> get iterator => DirEntriesIterator._(bindings, handle);
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
    return DirEntry._(bindings, bindings.dir_entries_get(handle, index));
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

class Error implements Exception {
  final String _message;

  Error(this._message);

  String toString() => _message;
}

class ErrorHelper {
  final ptr = malloc<Pointer<Int8>>();

  ErrorHelper();

  void check() {
    if (ptr.value != nullptr) {
      final error = ptr.value.cast<Utf8>().toDartString();
      // TODO: do we need to `free` the ptr here?
      throw Error(error);
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

