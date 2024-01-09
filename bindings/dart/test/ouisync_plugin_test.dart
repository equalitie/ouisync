import 'dart:convert';
import 'dart:io' as io;
import 'package:test/test.dart';
import 'package:ouisync_plugin/ouisync_plugin.dart';
import 'package:ouisync_plugin/state_monitor.dart';

void main() {
  late io.Directory temp;
  late Session session;
  late Repository repo;

  setUp(() async {
    temp = await io.Directory.systemTemp.createTemp();
    session = Session.create(configPath: '${temp.path}/config');
    repo = await Repository.create(
      session,
      store: '${temp.path}/repo.db',
      readPassword: null,
      writePassword: null,
    );
  });

  tearDown(() async {
    await repo.close();
    await session.close();
    await temp.delete(recursive: true);
  });

  test('file write and read', () async {
    final path = '/test.txt';
    final origContent = 'hello world';

    {
      final file = await File.create(repo, path);
      await file.write(0, utf8.encode(origContent));
      await file.close();
    }

    {
      final file = await File.open(repo, path);

      try {
        final length = await file.length;
        final readContent = utf8.decode(await file.read(0, length));

        expect(readContent, equals(origContent));
      } finally {
        await file.close();
      }
    }
  });

  test('empty directory', () async {
    final rootDir = await Directory.open(repo, '/');
    expect(rootDir, isEmpty);
  });

  test('share token access mode', () async {
    for (var mode in AccessMode.values) {
      final token = await repo.createShareToken(accessMode: mode);
      expect(await token.mode, equals(mode));
    }
  });

  test('parse invalid share token', () async {
    final input = "broken!@#%";
    expect(ShareToken.fromString(session, input), throwsA(isA<Error>()));
  });

  test('repository access mode', () async {
    expect(await repo.accessMode, equals(AccessMode.write));
  });

  test('repository sync progress', () async {
    final progress = await repo.syncProgress;
    expect(progress, equals(Progress(0, 0)));
  });

  test('repository state monitor', () async {
    expect(await repo.stateMonitor.load(), isNotNull);
  });

  test('state monitor missing node', () async {
    final monitor =
        session.rootStateMonitor.child(MonitorId.expectUnique("invalid"));
    final node = await monitor.load();
    expect(node, isNull);

    // This is to assert that no exception is thrown
    final monitorSubscription = monitor.subscribe();
    final streamSubscription = monitorSubscription.stream.listen((_) {});

    await streamSubscription.cancel();
    await monitorSubscription.close();
  });

  test('rename repository', () async {
    {
      final file = await File.create(repo, 'file.txt');
      await file.write(0, utf8.encode('hello world'));
      await file.close();
    }

    final token = await repo.createReopenToken();
    await repo.close();

    final src = '${temp.path}/repo.db';
    final dst = '${temp.path}/repo-new.db';

    for (final ext in ['', '-wal', '-shm']) {
      final file = io.File('$src$ext');

      if (await file.exists()) {
        await file.rename('$dst$ext');
      }
    }

    repo = await Repository.reopen(session, store: dst, token: token);

    {
      final file = await File.open(repo, 'file.txt');
      final content = await file.read(0, 11);
      expect(utf8.decode(content), equals('hello world'));
    }
  });

  test('user provided peers', () async {
    expect(await session.userProvidedPeers, isEmpty);

    final addr0 = 'quic/127.0.0.1:12345';
    final addr1 = 'quic/127.0.0.2:54321';

    await session.addUserProvidedPeer(addr0);
    expect(await session.userProvidedPeers, equals([addr0]));

    await session.addUserProvidedPeer(addr1);
    expect(await session.userProvidedPeers, equals([addr0, addr1]));

    await session.removeUserProvidedPeer(addr0);
    expect(await session.userProvidedPeers, equals([addr1]));

    await session.removeUserProvidedPeer(addr1);
    expect(await session.userProvidedPeers, isEmpty);
  });

  test('external addresses', () async {
    await session.bindNetwork(quicV4: '0.0.0.0:0', quicV6: '[::]:0');
    expect(await session.externalAddresses, isNotEmpty);
  },
      skip:
          'this test makes network requests to 3rd party services (use --run-skipped to force run)');
}
