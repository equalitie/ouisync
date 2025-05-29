import 'dart:async';

import 'package:async/async.dart';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:ouisync/ouisync.dart';
import 'package:ouisync/helpers.dart';
import 'package:path/path.dart';
import 'package:path_provider/path_provider.dart';

const _repoName = 'my repo';

void main() async {
  runApp(const MaterialApp(home: MyApp()));
}

class MyApp extends StatefulWidget {
  const MyApp({super.key});

  @override
  State<MyApp> createState() => _MyAppState();
}

class _MyAppState extends State<MyApp> {
  late Server server;
  late Session session;
  late Repository repo;

  bool bittorrentDhtEnabled = false;

  final contents = <String>[];

  @override
  void initState() {
    super.initState();
    unawaited(initObjects().then((value) => getFiles('/')));
  }

  Future<void> initObjects() async {
    final dataDir = (await getApplicationSupportDirectory()).path;
    final configDir = join(dataDir, 'config.db');

    final server = Server.create(
      configPath: configDir,
      notificationContentTitle: 'Ouisync example is running',
    )..initLog(stdout: true);
    await server.start();

    final session = await Session.create(configPath: configDir);
    await session.setStoreDir(join(dataDir, 'repos'));
    await session.initNetwork(NetworkDefaults(
      bind: ["quic/0.0.0.0:0"],
      portForwardingEnabled: false,
      localDiscoveryEnabled: false,
    ));

    Repository repo;

    try {
      repo = await session.findRepository(_repoName);
    } on NotFound catch (_) {
      repo = await session.createRepository(path: _repoName);
    }

    bittorrentDhtEnabled = await repo.isDhtEnabled();

    setState(() {
      this.session = session;
      this.repo = repo;
    });
  }

  @override
  void dispose() {
    unawaited(Future(() async {
      await server.stop();
      await session.close();
    }));

    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return DefaultTabController(
        length: 2,
        child: Scaffold(
            appBar: AppBar(
                title: const Text("Ouisync Example App"),
                bottom: const TabBar(
                    tabs: [Tab(text: "Files"), Tab(text: "Settings")])),
            body: TabBarView(
              children: [
                makeFileListBody(),
                makeSettingsBody(),
              ],
            )));
  }

  Widget makeFileListBody() {
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.all(4.0),
          child: Row(
            children: [
              ElevatedButton(
                  onPressed: () async =>
                      await addFile().then((value) => getFiles('/')),
                  child: const Text('Add file')),
            ],
          ),
        ),
        fileList(),
      ],
    );
  }

  Widget makeSettingsBody() {
    return Column(
      children: <Widget>[
        SwitchListTile(
          title: const Text("BitTorrent DHT"),
          value: bittorrentDhtEnabled,
          onChanged: (bool value) {
            setDhtEnabled(value);
          },
        ),
      ],
    );
  }

  Future<void> setDhtEnabled(bool enable) async {
    await repo.setDhtEnabled(enable);
    final isEnabled = await repo.isDhtEnabled();

    setState(() {
      bittorrentDhtEnabled = isEnabled;
    });
  }

  Widget fileList() => ListView.separated(
        separatorBuilder: (context, index) =>
            const Divider(height: 1, color: Colors.transparent),
        shrinkWrap: true,
        itemCount: contents.length,
        itemBuilder: (context, index) {
          final item = contents[index];

          return Card(
            child: ListTile(
              title: Text(item),
              onTap: () => showAlertDialog(context, item),
            ),
          );
        },
      );

  Future<void> addFile() async {
    FilePickerResult? result =
        await FilePicker.platform.pickFiles(withReadStream: true);

    if (result != null) {
      final path = '/${result.files.single.name}';
      final file = await createFile(path);
      await saveFile(file, path, result.files.first.readStream!);
    }
  }

  Future<File> createFile(String filePath) async {
    File? newFile;

    try {
      debugPrint('Creating file $filePath');
      newFile = await repo.createFile(filePath);
    } catch (e) {
      debugPrint('Error creating file $filePath: $e');
    }

    return newFile!;
  }

  Future<void> saveFile(
      File file, String path, Stream<List<int>> stream) async {
    debugPrint('Writing file $path');

    int offset = 0;

    try {
      final streamReader = ChunkedStreamReader(stream);
      while (true) {
        final buffer = await streamReader.readChunk(64000);
        debugPrint('Buffer size: ${buffer.length} - offset: $offset');

        if (buffer.isEmpty) {
          debugPrint('The buffer is empty; reading from the stream is done!');
          break;
        }

        await file.write(offset, buffer);
        offset += buffer.length;
      }
    } catch (e) {
      debugPrint('Exception writing the file $path:\n${e.toString()}');
    } finally {
      await file.close();
    }
  }

  Future<void> getFiles(String path) async {
    final dir = await repo.readDirectory(path);
    final items = dir.map((entry) => entry.name).toList();

    setState(() {
      contents.clear();
      contents.addAll(items);
    });
  }

  void showAlertDialog(BuildContext context, String path) {
    Widget previewFileButton = TextButton(
      child: const Text("Preview"),
      onPressed: () async {
        Navigator.of(context).pop();
        await viewFile(_getFileUrl(path));
      },
    );
    Widget shareFileButton = TextButton(
      child: const Text("Share"),
      onPressed: () async {
        Navigator.of(context).pop();
        await shareFile(_getFileUrl(path));
      },
    );
    Widget cancelButton = TextButton(
      child: const Text("Cancel"),
      onPressed: () {
        Navigator.of(context).pop();
      },
    );

    AlertDialog alert = AlertDialog(
      title: const Text("Ouisync Plugin Example App"),
      content: Text("File:\n$path"),
      actions: [
        previewFileButton,
        shareFileButton,
        cancelButton,
      ],
    );

    showDialog<AlertDialog>(
      context: context,
      builder: (BuildContext context) {
        return alert;
      },
    );
  }
}

Uri _getFileUrl(String path) => Uri(
      scheme: 'content',
      host: 'org.equalitie.ouisync.dart.example.provider',
      path: posix.join(_repoName, path),
    );
