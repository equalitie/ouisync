import 'dart:async';
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

  // Without a running server, a request fails after the connect timeout. The client stays usable,
  // so a later request succeeds once the server is running.
  test('connect timeout', () async {
    final configPath = '${temp.path}/config';
    final client = await Client.connect(
      configPath: configPath,
      connectTimeout: const Duration(milliseconds: 500),
    );

    await expectLater(
      client.invoke(RequestSessionGetStoreDirs()),
      throwsA(isA<io.FileSystemException>()),
    );

    // The failed request isn't left behind.
    expect(client.pendingRequestCount, 0);

    final server = Server.create(configPath: configPath);
    await server.start();

    try {
      expect(
        await client.invoke(RequestSessionGetStoreDirs()),
        isA<ResponsePaths>(),
      );
    } finally {
      await client.close();
      await server.stop();
    }
  });

  // Closing the client while it's trying to connect doesn't wait for the connection attempts to
  // succeed or time out, and the pending request fails.
  test('close while connecting', () async {
    final configPath = '${temp.path}/config';
    final client = await Client.connect(
      configPath: configPath,
      connectTimeout: null,
    );

    // No server is running, so this keeps trying to connect.
    final request = expectLater(
      client.invoke(RequestSessionGetStoreDirs()),
      throwsA(isA<StateError>()),
    );
    await Future<void>.delayed(const Duration(milliseconds: 200));

    await client.close().timeout(const Duration(seconds: 5));
    await request;
  });

  // A connection closed by the server during authentication (e.g. because the server is being
  // stopped) is a connection error, so it's retried like any other.
  test('connection closed during authentication', () async {
    final configPath = '${temp.path}/config';

    // Fake server which reads the client's challenge and then closes the connection. Reading it
    // first makes the close clean, so the client sees the end of the stream (not a reset).
    final server =
        await io.ServerSocket.bind(io.InternetAddress.loopbackIPv4, 0);
    server.listen((socket) {
      var received = 0;
      socket.listen((data) {
        received += data.length;

        if (received >= 256) {
          unawaited(socket.close());
        }
      });
    });

    await io.Directory(configPath).create(recursive: true);
    await io.File('$configPath/local_endpoint.conf').writeAsString(
      json.encode({'port': server.port, 'auth_key': '00' * 32}),
    );

    final client = await Client.connect(
      configPath: configPath,
      connectTimeout: const Duration(seconds: 1),
    );

    try {
      await expectLater(
        client.invoke(RequestSessionGetStoreDirs()),
        throwsA(isA<io.SocketException>()),
      );
    } finally {
      await client.close();
      await server.close();
    }
  });
}
