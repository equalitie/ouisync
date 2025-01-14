import 'dart:io' as io;
import 'package:test/test.dart';
import 'package:ouisync/ouisync.dart';

void main() {
  late io.Directory temp;

  setUp(() async {
    temp = await io.Directory.systemTemp.createTemp();
  });

  tearDown(() async {
    await temp.delete(recursive: true);
  });

  test('shared session', () async {
    final session0 = await Session.create(
      configPath: '${temp.path}/config',
    );

    final session1 = await Session.create(
      configPath: '${temp.path}/config',
    );

    try {
      final vs = await Future.wait([
        session0.currentProtocolVersion,
        session1.currentProtocolVersion,
      ]);

      expect(vs[0], equals(vs[1]));
    } finally {
      await session0.close();
      await session1.close();
    }
  });

  test('use after close', () async {
    final session = await Session.create(
      configPath: '${temp.path}/config',
    );
    await session.close();

    await expectLater(
        session.currentProtocolVersion, throwsA(isA<StateError>()));
  });
}
