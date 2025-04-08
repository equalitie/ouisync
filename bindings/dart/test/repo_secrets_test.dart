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
      configPath: '${temp.path}/config',
    );
    await session.setStoreDir('${temp.path}/store');
  });

  test('Open repo using keys', () async {
    final readSecret = SetLocalSecretKeyAndSalt(
      key: await session.generateSecretKey(),
      salt: await session.generatePasswordSalt(),
    );

    final writeSecret = SetLocalSecretKeyAndSalt(
      key: await session.generateSecretKey(),
      salt: await session.generatePasswordSalt(),
    );

    {
      final repo = await session.createRepository(
        path: repoName,
        readSecret: readSecret,
        writeSecret: writeSecret,
      );

      await repo.close();
    }

    {
      final repo = await session.openRepository(
        path: repoName,
        localSecret: LocalSecretSecretKey(readSecret.key),
      );

      expect(await repo.getAccessMode(), AccessMode.read);
      await repo.close();
    }

    {
      final repo = await session.openRepository(
        path: repoName,
        localSecret: LocalSecretSecretKey(writeSecret.key),
      );

      expect(await repo.getAccessMode(), AccessMode.write);
      await repo.close();
    }
  });

  test('Create repo using key, open with password', () async {
    final readPassword = Password('foo');
    final writePassword = Password('bar');

    {
      final readSalt = await session.generatePasswordSalt();
      final writeSalt = await session.generatePasswordSalt();

      final readKey = await session.deriveSecretKey(readPassword, readSalt);
      final writeKey = await session.deriveSecretKey(writePassword, writeSalt);

      final repo = await session.createRepository(
        path: repoName,
        readSecret: SetLocalSecretKeyAndSalt(
          key: readKey,
          salt: readSalt,
        ),
        writeSecret: SetLocalSecretKeyAndSalt(
          key: writeKey,
          salt: writeSalt,
        ),
      );

      await repo.close();
    }

    {
      final repo = await session.openRepository(
        path: repoName,
        localSecret: LocalSecretPassword(readPassword),
      );

      expect(await repo.getAccessMode(), AccessMode.read);
      await repo.close();
    }

    {
      final repo = await session.openRepository(
        path: repoName,
        localSecret: LocalSecretPassword(writePassword),
      );

      expect(await repo.getAccessMode(), AccessMode.write);
      await repo.close();
    }
  });

  tearDown(() async {
    await session.close();
    await temp.delete(recursive: true);
  });
}
