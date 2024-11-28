import 'dart:io' as io;
import 'package:test/test.dart';
import 'package:ouisync/ouisync.dart';

void main() {
  late io.Directory temp;
  late Session session;
  late Repository repo;

  setUp(() async {
    temp = await io.Directory.systemTemp.createTemp();
    session = await Session.create(
      socketPath: '${temp.path}/sock',
      configPath: '${temp.path}/config',
    );
    repo = await Repository.create(
      session,
      store: '${temp.path}/repo.db',
      readSecret: null,
      writeSecret: null,
    );
  });

  tearDown(() async {
    await session.close();
    await temp.delete(recursive: true);
  });

  test('Close a repository asynchronously in Windows fails', () async {
    await repo.close();
  });

  // test('Close a repository asynchronously  in Windows using microtask succeeds', () {
  //   Future.microtask(() async => await repo.close());
  // });
}
