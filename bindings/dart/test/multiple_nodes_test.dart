import 'dart:io' as io;

import 'package:test/test.dart';
import 'package:ouisync/ouisync.dart';

import 'utils.dart';

void main() {
  late io.Directory temp;

  late Server server1;
  late Session session1;
  late Repository repo1;

  late Server server2;
  late Session session2;
  late Repository repo2;

  setUp(() async {
    temp = await io.Directory.systemTemp.createTemp();

    await io.Directory('${temp.path}/1').create();
    await io.Directory('${temp.path}/2').create();

    final configPath1 = '${temp.path}/1/config';
    server1 = Server.create(configPath: configPath1, debugLabel: '1');
    await server1.start();
    session1 = await Session.create(configPath: configPath1);

    final configPath2 = '${temp.path}/2/config';
    server2 = Server.create(configPath: configPath2, debugLabel: '2');
    await server2.start();
    session2 = await Session.create(configPath: configPath2);

    await session1.setStoreDirs(['${temp.path}/1/store']);
    await session2.setStoreDirs(['${temp.path}/2/store']);

    repo1 = await session1.createRepository(
      path: 'repo1',
      readSecret: null,
      writeSecret: null,
    );
    await repo1.setSyncEnabled(true);
    final token = await repo1.share(accessMode: AccessMode.write);

    repo2 = await session2.createRepository(
      path: 'repo2',
      token: token,
      readSecret: null,
      writeSecret: null,
    );
    await repo2.setSyncEnabled(true);

    await session1.bindNetwork(["quic/127.0.0.1:0"]);
    await session2.bindNetwork(["quic/127.0.0.1:0"]);
  });

  tearDown(() async {
    await session2.close();
    await session1.close();

    await server2.stop();
    await server1.stop();

    await deleteTempDir(temp);
  });

  test('notification on sync', () async {
    // One event for each block created (one for the root directory and one for the file)
    final expect = expectLater(repo2.events, emitsInOrder([null, null]));

    final addrs = await session1.getLocalListenerAddrs();
    await session2.addUserProvidedPeers(addrs);

    final file = await repo1.createFile("file.txt");
    await file.close();

    await expect;
  });

  test('notification on peers change', () async {
    final addr =
        await session1.getLocalListenerAddrs().then((addrs) => addrs.first);

    final expect = expectLater(
      session2.networkEvents.asyncMap((_) => session2.getPeers()),
      emitsThrough(
        contains(
          isA<PeerInfo>()
              .having((peer) => peer.addr, 'addr', equals(addr))
              .having((peer) => peer.source, 'source',
                  equals(PeerSource.userProvided))
              .having((peer) => peer.state, 'state', isA<PeerStateActive>()),
        ),
      ),
    );

    await session2.addUserProvidedPeers([addr]);

    await expect;
  });

  test('network stats', () async {
    final addr =
        await session1.getLocalListenerAddrs().then((addrs) => addrs.first);
    await session2.addUserProvidedPeers([addr]);

    final file = await repo1.createFile('file.txt');
    await file.close();

    // Wait for the file to get synced
    while (true) {
      try {
        final file = await repo2.openFile('file.txt');
        await file.close();
        break;
      } catch (_) {}

      await repo2.events.first;
    }

    final stats = await session2.getNetworkStats();

    expect(stats.bytesTx, greaterThan(0));
    expect(stats.bytesRx, greaterThan(65536)); // at least two blocks received
  });
}
