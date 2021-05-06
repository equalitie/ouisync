import 'dart:ffi';
import 'package:ffi/ffi.dart';
import '../gen/ouisync_bindings.dart';

void main() {
  final bindings =
      OuisyncBindings(DynamicLibrary.open('../target/debug/libouisync.so'));

  final repo = bindings
      .open_repository('~/.local/share/ouisync/db'.toNativeUtf8().cast<Int8>());
  bindings.close_repository(repo);
}
