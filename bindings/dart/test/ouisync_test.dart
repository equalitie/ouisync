import 'dart:convert';
import 'dart:io' as io;
import 'package:test/test.dart';
import 'package:ouisync/ouisync.dart';
import 'package:ouisync/state_monitor.dart';

void main() {
  late io.Directory temp;
  late Session session;

  initLog(callback: (level, message) {
    // ignore: avoid_print
    print('${level.name.toUpperCase()} $message');
  });

  setUp(() async {
    temp = await io.Directory.systemTemp.createTemp();
    session = await Session.create(
      configPath: '${temp.path}/config',
    );

    final storeDir = '${temp.path}/store';
    await io.Directory(storeDir).create(recursive: true);
    await session.setStoreDir(storeDir);
  });

  tearDown(() async {
    await session.close();

    try {
      await temp.delete(recursive: true);
    } on io.PathNotFoundException {
      // TODO: Why does this sometimes happen?
    }
  });

  group('repository', () {
    final name = 'repo';
    late Repository repo;

    setUp(() async {
      repo = await session.createRepository(path: name);
    });

    test('list', () async {
      await repo.close();
      expect(await session.listRepositories(), isEmpty);

      repo = await session.openRepository(path: name);
      expect(await session.listRepositories().then((m) => m.values),
          equals([repo]));

      final repo2 = await session.createRepository(path: 'repo2');

      expect(await session.listRepositories().then((m) => m.values),
          unorderedEquals([repo, repo2]));
    });

    test('file write and read', () async {
      final path = '/test.txt';
      final origContent = 'hello world';

      {
        final file = await repo.createFile(path);
        await file.write(0, utf8.encode(origContent));
        await file.close();
      }

      {
        final file = await repo.openFile(path);

        try {
          final length = await file.getLength();
          final readContent = utf8.decode(await file.read(0, length));

          expect(readContent, equals(origContent));
        } finally {
          await file.close();
        }
      }
    });

    test('empty directory', () async {
      final rootDir = await repo.readDirectory('/');
      expect(rootDir, isEmpty);
    });

    test('directory create and remove', () async {
      expect(await repo.getEntryType('dir'), isNull);

      await repo.createDirectory('dir');
      expect(await repo.getEntryType('dir'), equals(EntryType.directory));

      await repo.removeDirectory('dir', false);
      expect(await repo.getEntryType('dir'), isNull);
    });

    test('share token access mode', () async {
      for (var mode in AccessMode.values) {
        final token = await repo.share(accessMode: mode);
        expect(await session.getShareTokenAccessMode(token), equals(mode));
      }
    });

    test('sync progress', () async {
      final progress = await repo.getSyncProgress();
      expect(progress, equals(Progress(total: 0, value: 0)));
    });

    test('state monitor', () async {
      expect(await repo.stateMonitor.then((s) => s.load()), isNotNull);
    });

    test('rename', () async {
      {
        final file = await repo.createFile('file.txt');
        await file.write(0, utf8.encode('hello world'));
        await file.close();
      }

      final cred = await repo.getCredentials();
      await repo.close();

      final base = await session.getStoreDir();

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

      repo = await session.openRepository(path: dstName);
      await repo.setCredentials(cred);

      {
        final file = await repo.openFile('file.txt');
        final content = await file.read(0, 11);
        expect(utf8.decode(content), equals('hello world'));
      }
    });

    test('get root directory contents after create and open', () async {
      expect(await repo.readDirectory('/'), equals([]));

      await repo.close();

      repo = await session.openRepository(path: name);

      expect(await repo.readDirectory('/'), equals([]));

      await repo.close();
    });

    test('access mode', () async {
      expect(await repo.getAccessMode(), equals(AccessMode.write));

      await repo.setAccessMode(AccessMode.read, null);
      expect(await repo.getAccessMode(), equals(AccessMode.read));

      await repo.setAccessMode(AccessMode.blind, null);
      expect(await repo.getAccessMode(), equals(AccessMode.blind));

      await repo.setAccessMode(AccessMode.write, null);
      expect(await repo.getAccessMode(), equals(AccessMode.write));
    });

    test('set access', () async {
      await repo.setAccess(
        read: AccessChangeEnable(
          value: SetLocalSecretPassword(value: Password('read_pass')),
        ),
        write: AccessChangeEnable(
          value: SetLocalSecretPassword(value: Password('write_pass')),
        ),
      );

      await repo.close();
      repo = await session.openRepository(path: name);
      expect(await repo.getAccessMode(), equals(AccessMode.blind));

      await repo.close();
      repo = await session.openRepository(
        path: name,
        localSecret: LocalSecretPassword(value: Password('read_pass')),
      );
      expect(await repo.getAccessMode(), equals(AccessMode.read));

      await repo.close();
      repo = await session.openRepository(
        path: name,
        localSecret: LocalSecretPassword(value: Password('write_pass')),
      );
      expect(await repo.getAccessMode(), equals(AccessMode.write));
    });

    test('metadata', () async {
      expect(await repo.getMetadata('test.foo'), isNull);
      expect(await repo.getMetadata('test.bar'), isNull);

      expect(
        await repo.setMetadata([
          MetadataEdit(
            key: 'test.foo',
            oldValue: null,
            newValue: 'foo value 1',
          ),
          MetadataEdit(
            key: 'test.bar',
            oldValue: null,
            newValue: 'bar value 1',
          ),
        ]),
        isTrue,
      );

      expect(await repo.getMetadata('test.foo'), equals('foo value 1'));
      expect(await repo.getMetadata('test.bar'), equals('bar value 1'));

      expect(
        await repo.setMetadata([
          MetadataEdit(
            key: 'test.foo',
            oldValue: 'foo value 1',
            newValue: 'foo value 2',
          ),
          MetadataEdit(
            key: 'test.bar',
            oldValue: 'bar value 1',
            newValue: null,
          ),
        ]),
        isTrue,
      );

      expect(await repo.getMetadata('test.foo'), equals('foo value 2'));
      expect(await repo.getMetadata('test.bar'), isNull);

      // Old value mismatch
      expect(
        await repo.setMetadata([
          MetadataEdit(
            key: 'test.foo',
            oldValue: 'foo value 1',
            newValue: 'foo value 3',
          ),
        ]),
        isFalse,
      );

      expect(await repo.getMetadata('test.foo'), equals('foo value 2'));
    });
  });

  test('parse invalid share token', () async {
    final input = "broken!@#%";
    await expectLater(
      session.validateShareToken(input),
      throwsA(isA<InvalidInput>()),
    );
  });

  test('state monitor missing node', () async {
    final monitor =
        session.rootStateMonitor.child(MonitorId.expectUnique("invalid"));
    final node = await monitor.load();
    expect(node, isNull);

    // This is to assert that no exception is thrown
    monitor.changes.listen((_) {});
  });

  test('user provided peers', () async {
    expect(await session.getUserProvidedPeers(), isEmpty);

    final addr0 = 'quic/127.0.0.1:12345';
    final addr1 = 'quic/127.0.0.2:54321';

    await session.addUserProvidedPeers([addr0]);
    expect(await session.getUserProvidedPeers(), equals([addr0]));

    await session.addUserProvidedPeers([addr1]);
    expect(await session.getUserProvidedPeers(), equals([addr0, addr1]));

    await session.removeUserProvidedPeers([addr0]);
    expect(await session.getUserProvidedPeers(), equals([addr1]));

    await session.removeUserProvidedPeers([addr1]);
    expect(await session.getUserProvidedPeers(), isEmpty);
  });

  group('stun', () {
    setUp(() async {
      await session.bindNetwork(['quic/0.0.0.0:0', 'quic/[::]:0']);
    });

    test('external address', () async {
      expect(await session.getExternalAddrV4(), isNotEmpty);
      expect(await session.getExternalAddrV6(), isNotEmpty);
    });

    test('nat behavior', () async {
      expect(
          await session.getNatBehavior(),
          anyOf(
            NatBehavior.endpointIndependent,
            NatBehavior.addressDependent,
            NatBehavior.addressAndPortDependent,
          ));
    });
  },
      skip:
          'these tests make network requests to 3rd party services (use --run-skipped to force run)');
}
