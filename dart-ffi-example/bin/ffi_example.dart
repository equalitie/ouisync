import 'dart:async';
import 'dart:collection';
import 'dart:ffi';
import 'dart:isolate';
import 'dart:math';
import 'package:ffi/ffi.dart';
import '../gen/bindings.dart';

Future<void> main() async {
  final session = await Session.open(
    '/home/adam/.local/share/ouisync/db',
    lib: DynamicLibrary.open('../target/debug/libouisync.so')
  );

  final repo = await Repository.open(session);
  final dir = await Directory.open(repo, '/');

  for (var entry in dir) {
    print('${entry.name}: ${entry.type}');
  }

  var file = await File.create(repo, 'file#${dir.length}.txt');
  await file.close();

  dir.close();
  repo.close();
  session.close();
}

class Session {
  final Bindings bindings;

  Session._(this.bindings);

  static Future<Session> open(String store, {DynamicLibrary? lib}) async {
    final bindings = Bindings(lib ?? _defaultLib());

    await withPool((pool) =>
      invoke<void>((port, error) =>
        bindings.session_open(
          NativeApi.postCObject.cast<Void>(),
          pool.toNativeUtf8(store),
          port,
          error)));

    return Session._(bindings);
  }

  void close() {
    bindings.session_close();
  }
}

class Repository {
  final Bindings bindings;
  final int handle;

  Repository._(this.bindings, this.handle);

  static Future<Repository> open(Session session) async {
    final bindings = session.bindings;
    return Repository._(bindings, await invoke<int>(
      (port, error) => bindings.repository_open(port, error)));
  }

  void close() {
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

class Directory with IterableMixin<DirEntry> {
  final Bindings bindings;
  final int handle;

  Directory._(this.bindings, this.handle);

  static Future<Directory> open(Repository repo, String path) async =>
    Directory._(repo.bindings, await withPool((pool) =>
      invoke<int>((port, error) =>
        repo.bindings.directory_open(repo.handle, pool.toNativeUtf8(path), port, error))));

  static Future<void> create(Repository repo, String path) =>
    withPool((pool) =>
      invoke<void>((port, error) => repo.bindings.directory_create(
        repo.handle, pool.toNativeUtf8(path), port, error)));

  void close() {
    bindings.directory_close(handle);
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
    : count = bindings.directory_num_entries(handle);

  @override DirEntry get current {
    assert(index >= 0 && index < count);
    return DirEntry._(bindings, bindings.directory_get_entry(handle, index));
  }

  @override
  bool moveNext() {
    index = min(index + 1, count);
    return index < count;
  }

}

class File {
  final Bindings bindings;
  final int handle;

  File._(this.bindings, this.handle);

  static Future<File> open(Repository repo, String path) async =>
    File._(repo.bindings, await withPool((pool) =>
      invoke<int>((port, error) =>
        repo.bindings.file_open(repo.handle, pool.toNativeUtf8(path), port, error))));

  static Future<File> create(Repository repo, String path) async =>
    File._(repo.bindings, await withPool((pool) =>
      invoke<int>((port, error) =>
        repo.bindings.file_create(repo.handle, pool.toNativeUtf8(path), port, error))));

  Future<void> close() =>
    invoke<void>((port, error) => bindings.file_close(handle, port, error));

  Future<void> flush() =>
    invoke<void>((port, error) => bindings.file_flush(handle, port, error));

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
  var ptr = malloc<Pointer<Int8>>();

  ErrorHelper();

  void check() {
    assert(ptr != nullptr);

    if (ptr.value != nullptr) {
      final error = ptr.value.cast<Utf8>().toDartString();

      // NOTE: we are freeing a pointer here that was allocated by the native side. This *should*
      // be fine as long as both sides are using the same allocator.
      malloc.free(ptr.value);
      malloc.free(ptr);
      ptr = nullptr;

      throw Error(error);
    }
  }
}

/// Allocator that tracks all allocations and frees them all at the same time.
class Pool implements Allocator {
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
  Pointer<Int8> toNativeUtf8(String str) => str.toNativeUtf8(allocator: this).cast<Int8>();
}

/// Call the function passing it a [Pool] which will be released when the function returns.
Future<T> withPool<T>(Future<T> Function(Pool) fun) async {
  final pool = Pool();

  try {
    return await fun(pool);
  } finally {
    pool.release();
  }
}

/// Helper to invoke a native async function.
Future<T> invoke<T>(void Function(int, Pointer<Pointer<Int8>>) fun) async {
  final error = ErrorHelper();
  final recvPort = ReceivePort();

  fun(recvPort.sendPort.nativePort, error.ptr);

  final result = await recvPort.first;

  recvPort.close();
  error.check();

  return result;
}
