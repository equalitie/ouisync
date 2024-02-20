import 'dart:io' as io;
import 'package:test/test.dart';
import 'package:ouisync_plugin/ouisync_plugin.dart';

void main() {
  late io.Directory temp;
  late Session session;
  late LocalSecretKeyAndSalt readSecret;
  late LocalSecretKeyAndSalt writeSecret;
  late String repoPath;

  setUp(() async {
    temp = await io.Directory.systemTemp.createTemp();

    repoPath = '${temp.path}/repo.db';

    session = Session.create(
      kind: SessionKind.unique,
      configPath: '${temp.path}/config',
    );

    readSecret =
        LocalSecretKeyAndSalt(LocalSecretKey.random(), PasswordSalt.random());

    writeSecret =
        LocalSecretKeyAndSalt(LocalSecretKey.random(), PasswordSalt.random());
  });

  test('Open repo in different ways', () async {
    {
      final repo = await Repository.create(
        session,
        store: repoPath,
        readSecret: readSecret,
        writeSecret: writeSecret,
      );

      await repo.close();
    }

    {
      final repo = await Repository.open(
        session,
        store: repoPath,
        secret: readSecret.key,
      );

      expect(await repo.accessMode, AccessMode.read);
      await repo.close();
    }

    {
      final repo = await Repository.open(
        session,
        store: repoPath,
        secret: writeSecret.key,
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
