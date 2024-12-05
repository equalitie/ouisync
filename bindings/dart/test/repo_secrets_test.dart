import 'dart:io' as io;
import 'package:test/test.dart';
import 'package:ouisync/ouisync.dart';

void main() {
  late io.Directory temp;
  late Session session;
  final repoName = 'repo';

  setUp(() async {
    temp = await io.Directory.systemTemp.createTemp();

    session = await Session.create(
      socketPath: '${temp.path}/sock',
      configPath: '${temp.path}/config',
    );
  });

  test('Open repo using keys', () async {
    final readSecret =
        LocalSecretKeyAndSalt(LocalSecretKey.random(), PasswordSalt.random());

    final writeSecret =
        LocalSecretKeyAndSalt(LocalSecretKey.random(), PasswordSalt.random());

    {
      final repo = await Repository.create(
        session,
        name: repoName,
        readSecret: readSecret,
        writeSecret: writeSecret,
      );

      await repo.close();
    }

    {
      final repo = await Repository.open(
        session,
        name: repoName,
        secret: readSecret.key,
      );

      expect(await repo.accessMode, AccessMode.read);
      await repo.close();
    }

    {
      final repo = await Repository.open(
        session,
        name: repoName,
        secret: writeSecret.key,
      );

      expect(await repo.accessMode, AccessMode.write);
      await repo.close();
    }
  });

  test('Create repo using key, open with password', () async {
    final readPassword = LocalPassword("foo");
    final writePassword = LocalPassword("bar");

    {
      final readSalt = await session.generatePasswordSalt();
      final writeSalt = await session.generatePasswordSalt();

      final readKey = await session.deriveSecretKey(readPassword, readSalt);
      final writeKey = await session.deriveSecretKey(writePassword, writeSalt);

      final repo = await Repository.create(
        session,
        name: repoName,
        readSecret: LocalSecretKeyAndSalt(readKey, readSalt),
        writeSecret: LocalSecretKeyAndSalt(writeKey, writeSalt),
      );

      await repo.close();
    }

    {
      final repo = await Repository.open(
        session,
        name: repoName,
        secret: readPassword,
      );

      expect(await repo.accessMode, AccessMode.read);
      await repo.close();
    }

    {
      final repo = await Repository.open(
        session,
        name: repoName,
        secret: writePassword,
      );

      expect(await repo.accessMode, AccessMode.write);
      await repo.close();
    }
  });

  tearDown(() async {
    await session.close();
    await temp.delete(recursive: true);
  });
}
