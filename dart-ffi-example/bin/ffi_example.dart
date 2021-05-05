import 'dart:ffi' as ffi;
import '../gen/ouisync_bindings.dart';

void main() {
    final bindings = OuisyncBindings(ffi.DynamicLibrary.open('../target/debug/libouisync.so'));
    bindings.hello_world();
}
