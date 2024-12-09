import 'dart:convert';
import 'dart:io' as io;
import 'package:ouisync/server.dart';
import 'package:test/test.dart';
import 'package:ouisync/ouisync.dart';
import 'package:ouisync/state_monitor.dart';

void main() {
  late io.Directory temp;
  late Session session;

  setUp(() async {
    logInit();

    temp = await io.Directory.systemTemp.createTemp();
    session = await Session.create(
      socketPath: '${temp.path}/sock',
      configPath: '${temp.path}/config',
    );
  });

  tearDown(() async {
    await session.close();
    await temp.delete(recursive: true);
  });

  group('repository', () {
    late String name;
    late Repository repo;

    setUp(() async {
      name = 'repo';
      repo = await Repository.create(
        session,
        name: name,
        readSecret: null,
        writeSecret: null,
      );
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
      final rootDir = await Directory.read(repo, '/');
      expect(rootDir, isEmpty);
    });

    test('share token access mode', () async {
      for (var mode in AccessMode.values) {
        final token = await repo.share(accessMode: mode);
        expect(await token.mode, equals(mode));
      }
    });

    test('sync progress', () async {
      final progress = await repo.syncProgress;
      expect(progress, equals(Progress(0, 0)));
    });

    test('state monitor', () async {
      expect(await repo.stateMonitor?.load(), isNotNull);
    });

    test('rename', () async {
      {
        final file = await File.create(repo, 'file.txt');
        await file.write(0, utf8.encode('hello world'));
        await file.close();
      }

      final cred = await repo.credentials;
      await repo.close();

      final base = await session.storeDir;

      final dstName = 'repo-new';
      final ext = 'ouisyncdb';
      final src = '$base/repo.$ext';
      final dst = '$base/$dstName.$ext';

      for (final suffix in ['', '-wal', '-shm']) {
        final file = io.File('$src$suffix');
        final exists = await file.exists();

        if (suffix.isEmpty) {
          expect(exists, isTrue, reason: '${file.path} does not exist');
        }

        if (exists) {
          await file.rename('$dst$suffix');
        }
      }

      repo = await Repository.open(session, name: dstName);
      await repo.setCredentials(cred);

      {
        final file = await File.open(repo, 'file.txt');
        final content = await file.read(0, 11);
        expect(utf8.decode(content), equals('hello world'));
      }
    });

    test('get root directory contents after create and open', () async {
      expect(await Directory.read(repo, '/'), equals([]));

      await repo.close();

      repo = await Repository.open(
        session,
        name: name,
        secret: null,
      );

      expect(await Directory.read(repo, '/'), equals([]));

      await repo.close();
    });

    test('access mode', () async {
      expect(await repo.accessMode, equals(AccessMode.write));

      await repo.setAccessMode(AccessMode.read);
      expect(await repo.accessMode, equals(AccessMode.read));

      await repo.setAccessMode(AccessMode.blind);
      expect(await repo.accessMode, equals(AccessMode.blind));

      await repo.setAccessMode(AccessMode.write);
      expect(await repo.accessMode, equals(AccessMode.write));
    });

    test('set access', () async {
      await repo.setAccess(
        read: EnableAccess(LocalPassword('read_pass')),
        write: EnableAccess(LocalPassword('write_pass')),
      );

      await repo.close();
      repo = await Repository.open(
        session,
        name: name,
      );
      expect(await repo.accessMode, equals(AccessMode.blind));

      await repo.close();
      repo = await Repository.open(
        session,
        name: name,
        secret: LocalPassword('read_pass'),
      );
      expect(await repo.accessMode, equals(AccessMode.read));

      await repo.close();
      repo = await Repository.open(
        session,
        name: name,
        secret: LocalPassword('write_pass'),
      );
      expect(await repo.accessMode, equals(AccessMode.write));
    });

    test('metadata', () async {
      expect(await repo.getMetadata('test.foo'), isNull);
      expect(await repo.getMetadata('test.bar'), isNull);

      expect(
        await repo.setMetadata({
          'test.foo': (oldValue: null, newValue: 'foo value 1'),
          'test.bar': (oldValue: null, newValue: 'bar value 1'),
        }),
        isTrue,
      );

      expect(await repo.getMetadata('test.foo'), equals('foo value 1'));
      expect(await repo.getMetadata('test.bar'), equals('bar value 1'));

      expect(
        await repo.setMetadata({
          'test.foo': (oldValue: 'foo value 1', newValue: 'foo value 2'),
          'test.bar': (oldValue: 'bar value 1', newValue: null),
        }),
        isTrue,
      );

      expect(await repo.getMetadata('test.foo'), equals('foo value 2'));
      expect(await repo.getMetadata('test.bar'), isNull);

      // Old value mismatch
      expect(
        await repo.setMetadata({
          'test.foo': (oldValue: 'foo value 1', newValue: 'foo value 3'),
        }),
        isFalse,
      );

      expect(await repo.getMetadata('test.foo'), equals('foo value 2'));
    });
  });

  test('parse invalid share token', () async {
    final input = "broken!@#%";
    await expectLater(
      ShareToken.fromString(session, input),
      throwsA(isA<InvalidData>()),
    );
  });

  test('state monitor missing node', () async {
    final monitor =
        session.rootStateMonitor.child(MonitorId.expectUnique("invalid"));
    final node = await monitor.load();
    expect(node, isNull);

    // This is to assert that no exception is thrown
    monitor.subscribe().listen((_) {});
  });

  test('user provided peers', () async {
    expect(await session.userProvidedPeers, isEmpty);

    final addr0 = 'quic/127.0.0.1:12345';
    final addr1 = 'quic/127.0.0.2:54321';

    await session.addUserProvidedPeers([addr0]);
    expect(await session.userProvidedPeers, equals([addr0]));

    await session.addUserProvidedPeers([addr1]);
    expect(await session.userProvidedPeers, equals([addr0, addr1]));

    await session.removeUserProvidedPeers([addr0]);
    expect(await session.userProvidedPeers, equals([addr1]));

    await session.removeUserProvidedPeers([addr1]);
    expect(await session.userProvidedPeers, isEmpty);
  });

  group('stun', () {
    setUp(() async {
      await session.bindNetwork(['quic/0.0.0.0:0', 'quic/[::]:0']);
    });

    test('external address', () async {
      expect(await session.externalAddressV4, isNotEmpty);
      expect(await session.externalAddressV6, isNotEmpty);
    });

    test('nat behavior', () async {
      expect(
          await session.natBehavior,
          anyOf(
            "endpoint independent",
            "address dependent",
            "address and port dependent",
          ));
    });
  },
      skip:
          'these tests make network requests to 3rd party services (use --run-skipped to force run)');
}
