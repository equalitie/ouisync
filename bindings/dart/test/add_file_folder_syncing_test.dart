import 'dart:async';
import 'dart:convert';
import 'dart:io' as io;

import 'package:ouisync_plugin/ouisync_plugin.dart';
import 'package:test/test.dart';

void main() {
  late io.Directory temp;
  late Session session;

  late Repository repository;
  late StreamSubscription<void> subscription;

  late String currentPath;

  final folder1Path = '/folder1';
  final file1InFolder1Path = '/folder1/file1.txt';
  final file1Content = 'Lorem ipsum dolor sit amet';

  Future<void> getDirectoryContents(Repository repo, String path) async {
    final folder1Contents = await Directory.open(repo, path);
    print('Directory contents: ${folder1Contents.toList()}');
  }

  setUp(() async {
    temp = await io.Directory.systemTemp.createTemp();
    session = Session.create(
      kind: SessionKind.unique,
      configPath: '${temp.path}/device_id.conf',
    );
    repository = await Repository.create(session,
        store: '${temp.path}/repo.db', readSecret: null, writeSecret: null);

    currentPath = '/';

    subscription = repository.events.listen((_) async {
      print('Syncing $currentPath');
      await getDirectoryContents(repository, currentPath);
    });
  });

  tearDown(() async {
    await subscription.cancel();
    await repository.close();
    await session.close();
    await temp.delete(recursive: true);
  });

  test('Add file to directory with syncing not in directory', () async {
    // Create folder1 (/folder1)
    {
      await Directory.create(repository, folder1Path);
      print('New folder: $folder1Path');
    }
    // Create file1.txt inside folder1 (/folder1/file1.txt)
    {
      print('About to create file $file1InFolder1Path');
      final file = await File.create(repository, file1InFolder1Path);
      await file.write(0, utf8.encode(file1Content));
      await file.close();
    }
    // Get contents of folder1 (/folder1) and confirm it contains only one entry
    {
      final folder1Contents = await Directory.open(repository, folder1Path);
      expect(folder1Contents.toList().length, equals(1));

      print('Folder1 contents: ${folder1Contents.toList()}');
    }
  });

  test('Add file with syncing in directory', () async {
    // Create folder1 (/folder1)
    {
      await Directory.create(repository, folder1Path);
      print('New folder: $folder1Path');

      currentPath = folder1Path;
    }
    // Create file1 inside folder1 (/folder1/file1.txt)
    {
      print('About to create new file $file1InFolder1Path');
      final file = await File.create(repository, file1InFolder1Path);
      await file.write(0, utf8.encode(file1Content));
      await file.close();
    }
    // Get contents of folder1 (/folder1) and confirm it contains only one entry
    {
      final folder1Contents = await Directory.open(repository, folder1Path);
      expect(folder1Contents.toList().length, equals(1));

      print('Folder1 contents: ${folder1Contents.toList()}');
    }
  });
}
