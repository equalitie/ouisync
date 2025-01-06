import 'dart:convert';
import 'dart:io' as io;

import 'package:ouisync/client.dart';
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
    final configPath = '${temp.path}/config';
    final server = await Server.start(configPath: configPath);
    final client = await Client.connect(configPath: configPath);

    expect(
      await client.invoke<String?>('repository_get_store_dir'),
      isNull,
    );

    await client.close();
    await server.stop();
  });

  test('connect timeout', () async {
    final configPath = '${temp.path}/config';

    await io.Directory(configPath).create(recursive: true);
    await io.File('$configPath/local_control_port.conf').writeAsString('0');

    await expectLater(
      Client.connect(
        configPath: configPath,
        timeout: const Duration(milliseconds: 500),
      ),
      throwsA(isA<io.SocketException>()),
    );
  });

  test('server already running', () async {
    logInit();

    final configPath = '${temp.path}/config';

    final server0 = await Server.start(
      configPath: configPath,
    );

    await expectLater(
      Server.start(configPath: configPath),
      throwsA(isA<ServiceAlreadyRunning>()),
    );

    final client = await Client.connect(configPath: configPath);

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
