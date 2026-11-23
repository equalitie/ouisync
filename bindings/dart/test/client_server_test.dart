import 'dart:convert';
import 'dart:io' as io;

import 'package:ouisync/ouisync.dart';
import 'package:ouisync/src/client.dart';
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
    final server = Server.create(configPath: configPath);
    await server.start();
    final client = await Client.connect(configPath: configPath);

    expect(
      await client.invoke(RequestSessionGetStoreDirs()),
      isA<ResponsePaths>(),
    );

    await client.close();
    await server.stop();
  });

  test('server already running', () async {
    final configPath = '${temp.path}/config';

    final server0 = Server.create(configPath: configPath);
    final server1 = Server.create(configPath: configPath);

    await server0.start();
    await expectLater(
      server1.start(),
      throwsA(isA<ServiceAlreadyRunning>()),
    );

    final client = await Client.connect(configPath: configPath);

    try {
      expect(
        await client.invoke(RequestSessionGetStoreDirs()),
        isA<ResponsePaths>(),
      );
    } finally {
      await client.close();
      await server0.stop();
    }
  });

  // When the server is restarted on a different port, the client reconnects to the new port (read
  // from the endpoint file).
  test('reconnect to a different port', () async {
    final configPath = '${temp.path}/config';
    final endpointFile = io.File('$configPath/local_endpoint.conf');

    final server = Server.create(configPath: configPath);
    await server.start();

    final client = await Client.connect(configPath: configPath);

    try {
      expect(
        await client.invoke(RequestSessionGetStoreDirs()),
        isA<ResponsePaths>(),
      );

      await server.stop();

      // Make the restarted server bind to a random (different) port. It then writes the new port
      // to the endpoint file.
      final endpoint =
          json.decode(await endpointFile.readAsString())
              as Map<String, Object?>;
      final oldPort = endpoint['port'];
      endpoint['port'] = 0;
      await endpointFile.writeAsString(json.encode(endpoint));

      await server.start();

      final newEndpoint =
          json.decode(await endpointFile.readAsString())
              as Map<String, Object?>;
      expect(newEndpoint['port'], isNot(oldPort));

      expect(
        await client
            .invoke(RequestSessionGetStoreDirs())
            .timeout(const Duration(seconds: 10)),
        isA<ResponsePaths>(),
      );
    } finally {
      // `close` hangs while the client is still trying to reconnect, so don't wait for it forever
      // (to report the actual failure instead of the test timing out).
      await client.close().timeout(
        const Duration(seconds: 5),
        onTimeout: () {},
      );
      await server.stop();
    }
  });
}
