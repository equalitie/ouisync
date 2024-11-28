import 'dart:async';
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
    final storePath = '${temp.path}/store';

    unawaited(runServer(
      socketPath: socketPath,
      configPath: '${temp.path}/config',
      storePath: storePath,
    ));

    final client = await SocketClient.connect(socketPath);

    try {
      expect(
        await client.invoke<String>('repository_get_store_dir'),
        equals(storePath),
      );
    } finally {
      await client.close();
    }
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

    final servers = [0, 1].map(
      (n) => runServer(
        socketPath: socketPath,
        configPath: '${temp.path}/config$n',
        storePath: '${temp.path}/store$n',
      )
          .then((_) => null as Object?, onError: (error) => error)
          .then((error) => (n, error)),
    );

    final (n, error) = await Future.any(servers);
    final expectedStorePath = '${temp.path}/store${1 - n}';

    // One of the servers should throw
    expect(error, isA<ServiceAlreadyRunning>());

    // The other should start normally. Verify it by connecting to it and making a simple request.
    final client = await SocketClient.connect(socketPath);

    try {
      expect(
        await client.invoke<String>('repository_get_store_dir'),
        equals(expectedStorePath),
      );
    } finally {
      await client.close();
    }
  });
}
