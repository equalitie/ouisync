/* A weird mixture of iocp.dart with sync.dart which uses callbacks and windows
events and abuses the dart event loop to finally make this work.

Like iocp.dart, it uses a single reactor running in a separate thread however
the reactor is implicitly implemented by windows: we specify a callback that
runs whenever an I/O operation completes

Like sync.dart, requests and responses are dispatched over flutter ports to
work around the windows limitation that callbacks can only ever be invoked by
the thread they were scheduled on. */
import 'dart:async';
import 'dart:ffi';
import 'dart:isolate';
import 'package:ffi/ffi.dart';
import 'package:win32/win32.dart';

/* This implementation uses the implicit windows event loop running inside an
isolate because it's fighting with the dart event loop. */
class NamedPipe {
  final int fd;
  final _overlapped = calloc<OVERLAPPED>(2); // memory needed by the kernel
  _Reactor? _reactor = _Reactor.current; // set to null on close
  NamedPipe._(this.fd);

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
    if(fd == INVALID_HANDLE_VALUE) { throw Exception("Open error"); }

    // By convention, the address of OVERLAPPED is stored in OVERLAPPED.hEvent
    final res = NamedPipe._(fd);
    res._overlapped[0].hEvent = res._overlapped.address;
    res._overlapped[1].hEvent = (res._overlapped + 1).address;
    return res;
  }

  void close() {
    if (_reactor == null) { return; } // make sure we didn't close twice (1)
    print("close called $fd ${_reactor!.refs}");

    // abort any outstanding requests
    late final err = StateError("Pipe closed");
    _reactor!.events.remove(_overlapped[0])?.completeError(err);
    _reactor!.events.remove(_overlapped[1])?.completeError(err);

    // notify kernel of abort
    CancelIoEx(fd, nullptr);
    CloseHandle(fd);
    calloc.free(_overlapped);

    _reactor!.unref(); // notify the reactor that this pipe no longer needs it
    _reactor = null; // make sure we didn't close twice (2)
  }

  /* Reads up to `length` bytes from the pipe and into `dst`, returning the
  number of bytes transferred. Blocks if no data is available in kernel buffers.
  
  A single system call is performed and `dst` is not always filled entirely. A
  return value of 0 signals that no more data will ever arrive (half closed).

  Throws a `PlatformException` on unrecoverable error. Interleaving calls to
  `read()` from multiple simultaneous coroutines is undefined behavior. */
  Future<int> read(Pointer<Uint8> dst, int length) =>
    _submit(_overlapped, dst, -length);

  /* Writes up to `length` bytes from `src` into the pipe, returning the number
  of bytes transferred. Blocks if the kernel buffers are completely full.
  
  A single system call is performed and `src` is rarely sent entirely, but at
  least 1 byte will always be written upon successful return.

  Throws a `PlatformException` on unrecoverable error. Interleaving calls to
  `write()` from multiple simultaneous coroutines is undefined behavior. */
  Future<int> write(Pointer<Uint8> src, int length) =>
    _submit(_overlapped + 1, src, length);

  /* Primes the reactor for `overlapped` (reading or writing) using `buff` of
  `size`, then waits for the result and returns the number of bytes transferred
  By convention, a negative size signals a read. */
  Future<int> _submit(Pointer<OVERLAPPED> overlapped, Pointer<Uint8> buff, int size) async {
    final reactor = _reactor;
    if (reactor == null) { throw StateError("Pipe closed"); }
    final events = reactor.events;
    if (events[overlapped] != null) { throw StateError("Concurrent access"); }
    final completer = events[overlapped] = Completer();
    try {
      (await reactor.req).send((fd, overlapped, buff, size));
      SetEvent(reactor.wakeup);
      return await completer.future;
    } finally {
      events.remove(overlapped);
    }
  }
}

class _Reactor {
  final int wakeup;
  final Map<Pointer<OVERLAPPED>, Completer<int>> events;
  final Future<SendPort> req;
  final ReceivePort res;
  late final Isolate worker;
  _Reactor(this.wakeup, this.events, this.req, this.res);
  int refs = 1; // when this ticks down to 0, the isolate is stopped

  // this isolate doesn't do much, so it doesn't make sense to have more than
  // one: we create it on demand and trash it when the last pipe is closed
  static _Reactor? _instance;
  static _Reactor get current {
    _Reactor? self = _instance;
    if (self == null || self.refs <= 0) {
      final wakeup = CreateEvent(nullptr, FALSE, FALSE, nullptr);
      if (wakeup == NULL) { throw Exception("Unable to create event"); }

      final req = Completer<SendPort>();
      Map<Pointer<OVERLAPPED>, Completer<int>> events = {};
      final res = ReceivePort(); res.listen((arg) {
        if (!req.isCompleted) {
          // the first message is the port that the isolate accepts requests on
          // we reply by sending it the event we use to signal new requests
          final port = arg as SendPort;
          req.complete(port);
          port.send(wakeup);
        } else  {
          final (fd, status) = arg;
          if (status is String) {
            events[fd]?.completeError(Exception(status));
          } else {
            events[fd]?.complete(status);
          } // ? only possible if closed
        }
      });
      self = _instance = _Reactor(wakeup, events, req.future, res);
      Isolate.spawn(_worker, res.sendPort).then((worker) {
        self!.worker = worker;
        self.unref();
      });
    }
    self.refs++;
    return self;
  }

  void unref() {
    if(--refs == 0) {
      print("reactor close");
      CloseHandle(wakeup);
      res.close();
      worker.kill();
    }
  }

  /* Runs in the child isolate, separated so it doesn't accidentally capture
  scope; after receiving an initial HANDLE treated as an interrupt, it accepts
  (OVERLAPPED, buff, size) tuples with size < 0 for reads and >= 0 for writes
  with 0 signalling a flush / fsync operation (not currently implemented!) */
  static void _worker(SendPort res) {
    // one-off pointer allocs for weird windows apis; presumably dart's gc will
    // flush these on isolate termination? otherwise, BRO, DO YOU EVEN STACK!?!
    final msg = calloc<MSG>();
    final wakeup = calloc<IntPtr>();
    wakeup.value = INVALID_HANDLE_VALUE;

    final iocb = NativeCallable<LPOVERLAPPED_COMPLETION_ROUTINE>.isolateLocal(
      (int err, int transferred, OVERLAPPED overlapped) =>
        res.send((Pointer<OVERLAPPED>.fromAddress(overlapped.hEvent),
                  err == 0 ? transferred : "Windows error $err"))
    ).nativeFunction;

    final req = ReceivePort(); req.listen((arg) {
      if (wakeup.value == INVALID_HANDLE_VALUE) {
        wakeup.value = arg;
      } else {
        final (fd, overlapped, buff, size) = arg as (int, Pointer<OVERLAPPED>, Pointer<Uint8>, int);
        if (size < 0) {
          ReadFileEx(fd, buff, -size, overlapped, iocb);
        } else {
          WriteFileEx(fd, buff, size, overlapped, iocb);
        }
      }
    });
    res.send(req.sendPort);

    void fail(String message) {
      print(message);
      req.close();
    }

    void poll() {
      // go to alertable sleep, letting windows trigger outstanding io callbacks 
      int status; do {
        status = MsgWaitForMultipleObjectsEx(1, wakeup, 500,
          QUEUE_STATUS_FLAGS.QS_ALLEVENTS,
          MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS.MWMO_ALERTABLE);
      } while (status == WAIT_EVENT.WAIT_IO_COMPLETION);

      switch (status) {
      // either our event was triggered by main (i.e. we have more i/o requests)
      // or we timed out (third *WaitFor* argument); let the dart ioloop process
      // any outstanding requests and schedule another wait later down the line 
      case WAIT_EVENT.WAIT_OBJECT_0:
      case WAIT_EVENT.WAIT_FAILED:
      case WAIT_EVENT.WAIT_TIMEOUT: Timer.run(poll);

      // we don't explicitly allocate a message queue but dart might so we have
      // to deal with this ourselves just in case (TODO: ditch this if not true)
      case WAIT_EVENT.WAIT_OBJECT_0 + 1:
        while (TRUE == PeekMessage(msg, NULL, 0, 0, PEEK_MESSAGE_REMOVE_TYPE.PM_REMOVE)) {
          print("dispatching event ${msg[0].message} for dart");
          if (msg[0].message == WM_QUIT) {
            PostQuitMessage(msg[0].wParam);
          } else {
            TranslateMessage(msg);
            DispatchMessage(msg);
          }
        }

      // this is bad and, because GetLastError doesn't work, we can't know why
      //case WAIT_EVENT.WAIT_FAILED:
      //  fail("namedpipe failed for unknowable reason");

      // this is bad because WE don't use mutexes at all so it shouldn't happen
      case WAIT_EVENT.WAIT_ABANDONED: fail("namedpipe failed due to some mutex");

      // this is bad because it's supposed to NEVER happen
      default: fail("namedpipe failed for undocumented reason $status");
      }
    }
    poll();
  }
}
