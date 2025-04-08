import 'dart:convert';
import 'dart:io' as io;

import 'package:flutter/foundation.dart';
import 'package:ouisync/ouisync.dart';
import 'package:test/test.dart';

void main() {
  late io.Directory temp;
  late Session session;
  late Repository repository;

  final folder1Path = '/folder1';
  final folder2Path = '/folder1/folder2';
  final folder2RootPath = '/folder2';
  final file1InFolder2Path = '/folder1/folder2/file1.txt';
  final fileContent = 'hello world';

  setUp(() async {
    temp = await io.Directory.systemTemp.createTemp();

    session = await Session.create(
      configPath: '${temp.path}/config',
    );
    await session.setStoreDir('${temp.path}/store');

    repository = await session.createRepository(
      path: 'repo',
      readSecret: null,
      writeSecret: null,
    );
  });

  tearDown(() async {
    await session.close();
    await temp.delete(recursive: true);
  });

  test('Move folder ok when folder to move is empty', () async {
    // Create folder1 (/folder1) and folder2 inside folder1 (/folder1/folder2)
    {
      await repository.createDirectory(folder1Path);
      debugPrint('New folder: $folder1Path');

      await repository.createDirectory(folder2Path);
      debugPrint('New folder: $folder2Path');
    }
    // Check that root (/) contains only one entry (/file1)
    {
      final rootContents = await repository.readDirectory('/');
      expect(rootContents.toList().length, equals(1));

      debugPrint('Root contents: ${rootContents.toList()}');
    }
    // Check that folder2 (/folder1/folder2) is empty
    {
      final rootContents = await repository.readDirectory(folder2Path);
      expect(rootContents.toList().length, equals(0));

      debugPrint('Folder2 contents: ${rootContents.toList()}');
    }
    // Move folder2 (/folder1/folder2) to root (/folder2)
    {
      debugPrint('Moving folder: src: $folder2Path - dst: $folder2RootPath');
      await repository.moveEntry(folder2Path, folder2RootPath);
    }
    // Check the contents in root for two entries: folder1 (/folder1) and folder2 (/folder2)
    {
      final rootContentsAfterMovingFolder2 =
          await repository.readDirectory('/');
      expect(rootContentsAfterMovingFolder2.isNotEmpty, equals(true));
      expect(rootContentsAfterMovingFolder2.toList().length, equals(2));

      debugPrint(
          'Root contents after move: ${rootContentsAfterMovingFolder2.toList()}');
    }
  });

  test('Move folder ok when folder to move is not empty', () async {
    // Create folder1 (/folder1) and folder2 inside folder1 (/folder1/folder2)
    {
      await repository.createDirectory(folder1Path);
      debugPrint('New folder: $folder1Path');

      await repository.createDirectory(folder2Path);
      debugPrint('New folder: $folder2Path');
    }
    // Check that root (/) contains only one entry (/file1)
    {
      final rootContents = await repository.readDirectory('/');
      expect(rootContents.toList().length, equals(1));

      debugPrint('Root contents: ${rootContents.toList()}');
    }
    // Create new file1.txt in folder2 (/folder1/folder2/file1.txt)
    {
      final file = await repository.createFile(file1InFolder2Path);
      await file.write(0, utf8.encode(fileContent));
      await file.close();
    }
    // Check that folder2 (/folder1/folder2) contains only one entry (/folder1/folder2/file1.txt)
    {
      final folder2Contents = await repository.readDirectory(folder2Path);
      expect(folder2Contents.toList().length, equals(1));

      debugPrint('Folder2 contents: ${folder2Contents.toList()}');
    }
    // Move folder2 (/folder1/folder2) to root (/folder2) when folder2 is not empty (/folder1/folder2/file1.txt)
    {
      debugPrint('Moving folder: src: $folder2Path - dst: $folder2RootPath');
      await repository.moveEntry(folder2Path, folder2RootPath);
    }
    // Check the contents in root for two entryes: folder1 (/folder1) and folder2 (/folder2)
    {
      final rootContentsAfterMovingFolder2 =
          await repository.readDirectory('/');
      expect(rootContentsAfterMovingFolder2.isNotEmpty, equals(true));
      expect(rootContentsAfterMovingFolder2.toList().length, equals(2));

      debugPrint(
          'Root contents after move: ${rootContentsAfterMovingFolder2.toList()}');
    }
  });
}
