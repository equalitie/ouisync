import 'dart:io' as io;

import 'package:ouisync/internal/socket_client.dart';
import 'package:ouisync/ouisync.dart';
import 'package:ouisync/server.dart';
import 'package:test/test.dart';

void main() {
  late io.Directory temp;

  setUp(() async {
    temp = await io.Directory.systemTemp.createTemp();
  });

  tearDown(() async {
    await temp.delete(recursive: true);
  });

  test('sanity check', () async {
    final socketPath = '${temp.path}/sock';

    final server = await Server.start(
      socketPath: socketPath,
      configPath: '${temp.path}/config',
    );

    final client = await SocketClient.connect(socketPath);

    expect(
      await client.invoke<String?>('repository_get_store_dir'),
      isNull,
    );

    await client.close();
    await server.stop();
  });

  test('connect timeout', () async {
    await expectLater(
      SocketClient.connect(
        '${temp.path}/sock',
        timeout: const Duration(milliseconds: 500),
      ),
      throwsException,
    );
  });

  test('server already running', () async {
    final socketPath = '${temp.path}/sock';

    logInit();

    final server0 = await Server.start(
      socketPath: socketPath,
      configPath: '${temp.path}/config0',
    );

    await expectLater(
      Server.start(
        socketPath: socketPath,
        configPath: '${temp.path}/config1',
      ),
      throwsA(isA<ServiceAlreadyRunning>()),
    );

    final client = await SocketClient.connect(socketPath);

    try {
      expect(
        await client.invoke<String?>('repository_get_store_dir'),
        isNull,
      );
    } finally {
      await client.close();
      await server0.stop();
    }
  });
}