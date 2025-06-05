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
    final configPath = '${temp.path}/config';

    final server = Server.create(configPath: configPath);
    await server.start();

    final session0 = await Session.create(configPath: configPath);
    final session1 = await Session.create(configPath: configPath);

    try {
      final vs = await Future.wait([
        session0.getCurrentProtocolVersion(),
        session1.getCurrentProtocolVersion(),
      ]);

      expect(vs[0], equals(vs[1]));
    } finally {
      await session0.close();
      await session1.close();
      await server.stop();
    }
  });

  test('use after close', () async {
    final configPath = '${temp.path}/config';

    final server = Server.create(configPath: configPath);

    try {
      await server.start();

      final session = await Session.create(configPath: configPath);
      await session.close();

      await expectLater(
        session.getCurrentProtocolVersion(),
        throwsA(isA<StateError>()),
      );
    } finally {
      await server.stop();
    }
  });
}
