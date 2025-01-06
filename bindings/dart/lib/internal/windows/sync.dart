/* A (much) less efficient version of iocp.dart, though possibly sufficient
if the number of pipes is small and dart was being honest about its 30kib per
isolate memory usage. This can't be used either because windows files are only
full duplex when accessed asynchronously!

https://stackoverflow.com/questions/58483293/behavior-of-win32-named-pipe-in-duplex-mode
*/
import 'dart:async';
import 'dart:ffi';
import 'dart:isolate';
import 'package:ffi/ffi.dart';
import 'package:win32/win32.dart';

/* This implementation spawns two isolates per pipe and sends them the fd to
read from and respectively write to.

While the problem of not being able to meaningfully call `GetLastError()`
persists, it's no longer critical in this case as there is no `PENDING` status
to disambiguate behavior on. Instead, errors are coalesced based on their most
recent call, hiding their root cause but not their effects. */
class NamedPipe {
  final int fd; // the windows handle to the open pipe
  final RPC<int, Pointer<Uint32>, Req, int> _read, _write;
  NamedPipe._(this.fd, this._read, this._write);

  Future<int> read(Pointer<Uint8> dst, int length) => _read((dst, length));
  Future<int> write(Pointer<Uint8> src, int length) => _write((src, length));

  void close() {
    _read.dispose();
    _write.dispose();
    CloseHandle(fd);
  }

  static Future<NamedPipe> open(String path) async {
    final ptr = path.toNativeUtf16(allocator: calloc);
    final fd = CreateFile(ptr,
                          GENERIC_ACCESS_RIGHTS.GENERIC_READ
                        | GENERIC_ACCESS_RIGHTS.GENERIC_WRITE,
                          0, nullptr,
                          FILE_CREATION_DISPOSITION.OPEN_EXISTING,
                          FILE_FLAGS_AND_ATTRIBUTES.FILE_ATTRIBUTE_NORMAL
                        | FILE_FLAGS_AND_ATTRIBUTES.FILE_FLAG_WRITE_THROUGH,
                          NULL);
    calloc.free(ptr);
    if (fd == INVALID_HANDLE_VALUE) {
      throw Exception("Open error");
    }
    final reader = RPC(fd, () => calloc<Uint32>(), (int fd, Pointer<Uint32> count, Req arg) {
      if(FALSE == ReadFile(fd, arg.$1, arg.$2, count, nullptr)) {
        throw Exception("Read error");
      }
      return count.value;
    });
    final writer = RPC(fd, () => calloc<Uint32>(), (int fd, Pointer<Uint32> count, Req arg) {
      if(FALSE == WriteFile(fd, arg.$1, arg.$2, count, nullptr)) {
        throw Exception("Write error");
      }
      return count.value;
    });
    return NamedPipe._(fd, reader, writer);
  }
}
// a (ptr, len) record used as the argument to a read / write call because dart
// does not support varargs
typedef Req = (Pointer<Uint8>, int);


/* Runs a serial synchronous callback within an isolate. Due to some suboptimal
behavior regarding dart closures https://github.com/dart-lang/sdk/issues/36983 ,
the callback must take EXACTLY three arguments:
1. some `state` delivered from the calling isolate on init on startup
2. the result of a `init` function called in the child isolate on init
3. the argument used for the actual invocation, passed from the calling isolate
   (varargs not supported: https://github.com/dart-lang/language/issues/1014 )

The isolate will be killed when `dispose()` is called. A previous version used 
finalizers: https://api.flutter.dev/flutter/dart-core/Finalizer-class.html but
was scrapped because they are not guaranteed to run and an active ReceivePort
prevents the main (caller) isolate from shutting down gracefully.

Please note that Dart imposes some limitations on S, I and O:
https://api.dart.dev/dart-isolate/SendPort/send.html */
class RPC<S,L,I,O> {
  final Future<SendPort> _req;
  final ReceivePort _res;
  Future<Isolate>? _worker;
  Completer<(O?, Object?)>? _next;
  RPC._(this._req, this._res, this._worker);
  factory RPC(S state, L Function() init, O Function(S,L,I) cb) {
    final res = ReceivePort();
    final isolate = Isolate.spawn((res) {
      final req = ReceivePort();
      S? state;
      L local = init();
      req.listen((arg) {
        if (state != null) {
          try { res.send((cb(state as S, local, arg), null)); }
          catch(e) { res.send((null, e)); }
        } else { state = arg; }
      });
      res.send(req.sendPort);
    }, res.sendPort);
    final req = Completer<SendPort>();
    final self = RPC<S,L,I,O>._(req.future.then((req) {
      req.send(state);
      return req;
    }), res, isolate);
    res.listen((arg) {
      if (req.isCompleted) { self._next?.complete(arg); }
      else { req.complete(arg); }
    });
    return self;
  }

  Future<O> call(I arg) async {
    if(_worker == null) { throw StateError("RPC instance disposed"); }
    if(_next != null) { throw StateError("Concurrent use of RPC"); }
    _next = Completer();
    (await _req).send(arg);
    final (res, err) = await _next!.future;
    _next = null;
    if (res == null) { throw err!; }
    return res;
  }

  void dispose() {
    if(_worker == null) { return; }
    _worker!.then((isolate) => isolate.kill(priority: Isolate.immediate));
    _worker = null;
    _res.close();
    _next?.complete((null, StateError("RPC instance disposed")));
    _next = null;
  }
}
