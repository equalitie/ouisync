import 'dart:io' as io;

import 'package:ouisync_plugin/ouisync_plugin.dart';
import 'package:test/test.dart';

void main() {
  late Session session;
  Repository? repository;

  final appDirectory = 'test/stores';
  final sessionStore = '$appDirectory/config.db';
  final repositoryStore = '$appDirectory/repo.db';

  final path = '/';

  setUp(() async {
    await io.Directory(appDirectory).create();

    session = Session.create(configPath: sessionStore);
  });

  tearDown(() async {
    await repository?.close();
    await session.close();

    await io.Directory(appDirectory).delete(recursive: true);
  });

  test(
      'Get root directory contents successfuly after Repository.create(...); fail when Repository.open(...)',
      () async {
    {
      repository = await Repository.create(session,
          store: repositoryStore, readPassword: null, writePassword: null);

      await getDirectoryContents(repository!, path);

      await repository?.close();
      repository = null;
    }
    {
      repository = await Repository.open(session,
          store: repositoryStore, password: null);

      await getDirectoryContents(repository!, path);
    }
  });
}

Future<void> getDirectoryContents(Repository repository, String path) async {
  final contents = await Directory.open(repository, path);
  expect(contents.toList().length, equals(0));

  print('Root contents: ${contents.toList()}');
}
