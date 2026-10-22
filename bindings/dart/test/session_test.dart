import 'dart:async';
import 'dart:io' as io;
import 'package:async/async.dart' show StreamQueue;
import 'package:test/test.dart';
import 'package:ouisync/ouisync.dart';

void main() {
  late io.Directory temp;

  setUp(() async {
    temp = await io.Directory.systemTemp.createTemp();
  });

  tearDown(() async {
    await temp.delete(recursive: true);
  });

  test('shared session', () async {
    final configPath = '${temp.path}/config';

    final server = Server.create(configPath: configPath);
    await server.start();

    final session0 = await Session.create(configPath: configPath);
    final session1 = await Session.create(configPath: configPath);

    try {
      final vs = await Future.wait([
        session0.getCurrentProtocolVersion(),
        session1.getCurrentProtocolVersion(),
      ]);

      expect(vs[0], equals(vs[1]));
    } finally {
      await session0.close();
      await session1.close();
      await server.stop();
    }
  });

  test('use after close', () async {
    final configPath = '${temp.path}/config';

    final server = Server.create(configPath: configPath);

    try {
      await server.start();

      final session = await Session.create(configPath: configPath);
      await session.close();

      await expectLater(
        session.getCurrentProtocolVersion(),
        throwsA(isA<StateError>()),
      );
    } finally {
      await server.stop();
    }
  });

  test('subscription survives the connection being dropped and reestablished',
      () async {
    final configPath = '${temp.path}/config';

    var server = Server.create(configPath: configPath);
    await server.start();

    final session = await Session.create(configPath: configPath);
    final storeDir = '${temp.path}/store';
    await io.Directory(storeDir).create(recursive: true);
    await session.setStoreDirs([storeDir]);

    try {
      // "Repositories" already exists by the time we subscribe (the service creates it at
      // startup), so from here on it only fires on repository creation/removal, which gives
      // us a deterministic way to tell whether the subscription is still alive.
      final repositories =
          session.rootStateMonitor.child(MonitorId.expectUnique('Repositories'));
      final changes = StreamQueue(repositories.changes);

      await session.createRepository(path: 'before');
      await changes.next.timeout(const Duration(seconds: 5));

      // Simulate the connection being severed while the service keeps running underneath
      // (e.g. iOS tearing down the app's sockets while it's backgrounded) by bouncing the
      // server without touching the client. It rebinds to the same port (persisted in the
      // shared config), so the already-connected `Client` just sees its socket die.
      await server.stop();
      server = Server.create(configPath: configPath);
      await server.start();

      // The subscription must keep delivering updates once the client reconnects, without
      // the caller having to resubscribe.
      await session.createRepository(path: 'after');
      await changes.next.timeout(const Duration(seconds: 5));

      await changes.cancel();
    } finally {
      await session.close();
      await server.stop();
    }
  });

  test('reports the service as unresponsive when nothing is listening '
      'anymore', () async {
    final configPath = '${temp.path}/config';

    final server = Server.create(configPath: configPath);
    await server.start();

    final session = await Session.create(configPath: configPath);

    try {
      // An active subscription is what drives reconnect attempts once the connection drops -
      // an idle client only reconnects lazily on its next call.
      final subscription = session.rootStateMonitor.changes.listen((_) {});
      await Future.delayed(const Duration(milliseconds: 200));

      // Unlike the "subscription survives..." test above, don't bring the service back up right
      // away: this simulates the native service process/thread dying outright (observed after
      // some iOS background/foreground cycles) rather than just having its socket torn down, so
      // every reconnect attempt keeps getting refused until something actively restarts it.
      await server.stop();

      await expectLater(
        session.serviceUnresponsive.first.timeout(const Duration(seconds: 10)),
        completes,
      );

      // Bring the service back so the client's in-flight reconnect attempt can succeed; without
      // this, `session.close()` below would hang forever waiting for a reconnect that can never
      // complete.
      await server.start();

      await subscription.cancel();
    } finally {
      await session.close();
      await server.stop();
    }
  });
}
