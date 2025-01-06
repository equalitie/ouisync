/* This file cannot be used until https://github.com/dart-lang/sdk/issues/38832
is fixed. Even then, it might be better to implement authentication and switch
to sockets because Java is apparently having trouble with domain sockets too
and maintaining a local reactor just for this might be overkill.

The problem is not _just_ about not being able to show exactly what's wrong,
async sockets throw `WIN32_ERROR.ERROR_IO_PENDING` to signal future completion.
WARN: due to the above, this implementation could not have been tested and
should be considered HIGHLY EXPERIMENTAL even if the issue does get fixed. */
import 'dart:async';
import 'dart:isolate';
import 'dart:ffi';
import 'package:ffi/ffi.dart';
import 'package:flutter/services.dart';
import 'package:win32/win32.dart';

/* returns a `PlatformException` associated with `code`, using the system
provided error message translated into the user's default locale */
PlatformException _error(int code) {
  String message = "Unknown windows error";
  Pointer<Utf16> description = nullptr;
  FormatMessage(FORMAT_MESSAGE_OPTIONS.FORMAT_MESSAGE_FROM_SYSTEM
              | FORMAT_MESSAGE_OPTIONS.FORMAT_MESSAGE_ALLOCATE_BUFFER
              | FORMAT_MESSAGE_OPTIONS.FORMAT_MESSAGE_IGNORE_INSERTS,
                nullptr, code, GetUserDefaultLangID(), description, NULL, nullptr);
  if (description != nullptr) {
    message = description.toDartString();
    LocalFree(description);
  }

  return PlatformException(code: "WIN$code", message: message);
}

/* If `cond` is false, throws a `PlatformException` derived from the most
recent windows error raised on the current thread */
void _assert(bool cond) {
  if (!cond) { throw _error(GetLastError()); }
}

/* This implementation uses a custom event loop running inside an isolate */
class NamedPipe {
  final int fd; // the windows handle to the open pipe
  _Reactor? _reactor; // set to null on close; keeps reactor active while open
  final _overlapped = calloc<OVERLAPPED>(2); // memory needed by the kernel
  NamedPipe._(this.fd, this._reactor);

  static Future<NamedPipe> open(String path) async {
    // open file
    final ptr = path.toNativeUtf16(allocator: calloc);
    final fd = CreateFile(ptr,
                          GENERIC_ACCESS_RIGHTS.GENERIC_READ
                        | GENERIC_ACCESS_RIGHTS.GENERIC_WRITE,
                          FILE_SHARE_MODE.FILE_SHARE_READ
                        | FILE_SHARE_MODE.FILE_SHARE_WRITE,
                          nullptr,
                          FILE_CREATION_DISPOSITION.OPEN_EXISTING,
                          FILE_FLAGS_AND_ATTRIBUTES.FILE_ATTRIBUTE_NORMAL
                        | FILE_FLAGS_AND_ATTRIBUTES.FILE_FLAG_OVERLAPPED
                        | FILE_FLAGS_AND_ATTRIBUTES.FILE_FLAG_WRITE_THROUGH,
                          NULL);
    calloc.free(ptr);
    _assert(fd != INVALID_HANDLE_VALUE);

    // get (or create) active reactor and associate it with the newly opened
    // file handle; the association is undone automatically by CloseHandle
    final reactor = _Reactor.current;
    final iocp = await reactor.fd;
    _assert(CreateIoCompletionPort(fd, iocp, NULL, NULL) == iocp);
    return NamedPipe._(fd, reactor);
  }

  void close() {
    if (_reactor == null) { return; } // make sure we didn't close twice (1)
  
    // abort any outstanding requests
    late final err = _error(WIN32_ERROR.ERROR_OPERATION_ABORTED);
    _reactor!.events.remove(_overlapped.address)?.completeError(err);
    _reactor!.events.remove((_overlapped+1).address)?.completeError(err);

    CancelIoEx(fd, nullptr);
    CloseHandle(fd);

    _reactor!.unref(); // notify the reactor that this pipe no longer needs it
    _reactor = null; // make sure we didn't close twice (2)

    // free memory and os handles; WARN: there's a potential race condition
    // here if the allocator immediately reuses this exact same memory address
    calloc.free(_overlapped);
  }

  /* Reads up to `length` bytes from the pipe and into `dst`, returning the
  number of bytes transferred. Blocks if no data is available in kernel buffers.
  
  A single system call is performed and `dst` is not always filled entirely. A
  return value of 0 signals that no more data will ever arrive (half closed).

  Throws a `PlatformException` on unrecoverable error. Interleaving calls to
  `read()` from multiple simultaneous coroutines is undefined behavior. */
  Future<int> read(Pointer<Uint8> dst, int length) => _submit(_overlapped,
    () => ReadFile(fd, dst, length, nullptr, _overlapped));

  /* Writes up to `length` bytes from `src` into the pipe, returning the number
  of bytes transferred. Blocks if the kernel buffers are completely full.
  
  A single system call is performed and `src` is rarely sent entirely, but at
  least 1 byte will always be written upon successful return.

  Throws a `PlatformException` on unrecoverable error. Interleaving calls to
  `write()` from multiple simultaneous coroutines is undefined behavior. */
  Future<int> write(Pointer<Uint8> src, int length) => _submit(_overlapped + 1,
    () => WriteFile(fd, src, length, nullptr, _overlapped + 1));

  /* Primes the reactor for `overlapped` (reading or writing), runs `cb()`, then
  wait for the result (if the opration will be completed asyncronously) and
  returns the number of bytes transferred via the `overlapped` structure. */
  Future<int> _submit(Pointer<OVERLAPPED> overlapped, int Function() cb) async {
    final events = _reactor?.events;
    if (events == null) { throw _error(WIN32_ERROR.ERROR_INVALID_HANDLE); }
    if (events[overlapped.address] != null) {
      // this error is something about printers, but that's what UB gets you!
      throw _error(WIN32_ERROR.ERROR_ALREADY_WAITING);
    }
    final completer = events[overlapped.address] = Completer();
    try {
      if (cb() == FALSE) {
        final code = GetLastError();
        if (code != WIN32_ERROR.ERROR_IO_PENDING) { throw _error(code); }
        await completer.future;
        _assert(GetOverlappedResult(fd, overlapped, nullptr, FALSE) != FALSE);
      }
      return overlapped.ref.InternalHigh;
    } finally {
      events.remove(overlapped.address);
    }
  }
}

class _Reactor {
  /* OVERLAPPED traditionally uses CreateEvent() which we can't use because it's
  blocking; ReadFileEx accepts a callback, but that only runs when the thread
  that called it is in "alertable sleep"; "IO completion ports" is the only
  known signalling mechanism that is not restricted to the same thread, but not
  to worry: dart is helpfully covering the gap here by making sure that
  Completer is non-sendable. So we're left with addr(OVERLAPPED) => Completer,
  with the isolate sending us addr(OVERLAPPED) over the wire */
  Map<int, Completer<void>> events;

  Future<int> fd; // the IOCP handle (mostly works like an epoll fd)
  _Reactor(this.events, this.fd, this.receiver);
  int refs = 1; // when this ticks down to 0, the isolate is stopped
  final ReceivePort receiver;
  late final Isolate worker;

  // this isolate doesn't do much, so it doesn't make sense to have more than
  // one: we create it on demand and trash it when the last pipe is closed
  static _Reactor? _instance;
  static _Reactor get current {
    _Reactor? self = _instance;
    if (self == null || self.refs <= 0) {
      final fd = Completer<int>();
      Map<int, Completer<void>> events = {};
      final receiver = ReceivePort(); receiver.listen((arg) {
        if (fd.isCompleted) {
          // this is the pointer to a completed OVERLAPPED request which was
          // added to events by NamedPipe prior to issuing the async call
          events[arg]?.complete(); // ? is only possible if closed
        } else {
          // the first message received is the fd of the IOCP
          fd.complete(arg);
        }
      });
      self = _instance = _Reactor(events, fd.future, receiver);
      Isolate.spawn(_listen, receiver.sendPort).then((worker) {
        self!.worker = worker;
        self.unref();
      });
    }
    self.refs++;
    return self;
  }

  void unref() {
    if(--refs == 0) {
      receiver.close();
      worker.kill();
    }
  }

  // this runs in the child isolate; while conceptually it might belong in
  // `get current`, it's extracted as a static method so it doesn't accidentally
  // capture scope; please make sure to only send ints over the `port`
  static void _listen(SendPort port) {
    final fd = CreateIoCompletionPort(INVALID_HANDLE_VALUE, NULL, NULL, 1);
    _assert(fd != INVALID_HANDLE_VALUE);
    port.send(fd);

    final max = 128; // events to process / syscall: 128 should fit in one page
    final entries = calloc<OVERLAPPED_ENTRY>(max);
    final triggered = calloc<Uint32>();
    while (true) {
      _assert(GetQueuedCompletionStatusEx(fd, entries, max, triggered, 1000, FALSE)
              == WIN32_ERROR.NO_ERROR);
      for (int i=0; i<triggered.value; i++) {
        port.send(entries[i].lpOverlapped.address);
      }
    }
  }
}
