import 'dart:convert' show json;
import 'dart:io' as io;
import 'dart:math' show Random;

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

  // Run sanity check with the default API protocol transport (unix domain socket on platforms that
  // support it, TCP on loopback otherwise)
  test('sanity check default', () => _sanityCheck('${temp.path}/config'));

  // Run sanity check with TCP on loopback as the API protocol transport.
  test('sanity check tcp', () async {
    final configPath = '${temp.path}/config';

    final conf = io.File('$configPath/local_endpoint.conf');
    await conf.create(recursive: true);
    await conf.writeAsString(
      json.encode(
        Uri(
          scheme: 'tcp',
          host: '127.0.0.1',
          port: 0,
          queryParameters: {'auth_key': _randomHex(64)},
        ).toString(),
      ),
    );

    await _sanityCheck(configPath);
  });

  test('server already running', () async {
    final configPath = '${temp.path}/config';

    final server0 = Server.create(configPath: configPath);
    final server1 = Server.create(configPath: configPath);

    await server0.start();
    await expectLater(server1.start(), throwsA(isA<ServiceAlreadyRunning>()));

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
}

Future<void> _sanityCheck(String configPath) async {
  final server = Server.create(configPath: configPath);
  await server.start();
  final client = await Client.connect(configPath: configPath);

  expect(
    await client.invoke(RequestSessionGetStoreDirs()),
    isA<ResponsePaths>(),
  );

  await client.close();
  await server.stop();
}

String _randomHex(int length) {
  const alphabet = '0123456789abcdef';
  final rng = Random();
  return List.generate(
    length,
    (_) => alphabet[rng.nextInt(alphabet.length)],
  ).join();
}
