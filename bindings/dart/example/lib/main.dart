import 'dart:io' as io;
import 'dart:async';

import 'package:async/async.dart';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:ouisync/ouisync.dart';
import 'package:ouisync/native_channels.dart';
import 'package:path/path.dart';
import 'package:path_provider/path_provider.dart';

void main() async {
  runApp(const MaterialApp(home: MyApp()));
}

class MyApp extends StatefulWidget {
  const MyApp({super.key});

  @override
  State<MyApp> createState() => _MyAppState();
}

class _MyAppState extends State<MyApp> {
  late Session session;
  late Repository repo;
  late NativeChannels nativeChannels;

  bool bittorrentDhtEnabled = false;

  final contents = <String>[];

  @override
  void initState() {
    super.initState();
    unawaited(initObjects().then((value) => getFiles('/')));
  }

  Future<void> initObjects() async {
    final dataDir = (await getApplicationSupportDirectory()).path;
    final session = await Session.create(configPath: join(dataDir, 'config.db'));

    final repoPath = join(dataDir, 'repo.db');
    final repoExists = await io.File(repoPath).exists();

    final repo = repoExists
        ? await Repository.open(session, path: repoPath, secret: null)
        : await Repository.create(
            session,
            path: repoPath,
            readSecret: null,
            writeSecret: null,
          );

    bittorrentDhtEnabled = await repo.isDhtEnabled;

    final nativeChannels = NativeChannels();
    nativeChannels.repository = repo;

    setState(() {
      this.session = session;
      this.repo = repo;
      this.nativeChannels = nativeChannels;
    });
  }

  @override
  void dispose() {
    repo.close();
    unawaited(session.close());

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
    final isEnabled = await repo.isDhtEnabled;

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
                onTap: () => showAlertDialog(context, item, 1)));
      });

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
      if (kDebugMode) {
        print('Creating file $filePath');
      }
      newFile = await File.create(repo, filePath);
    } catch (e) {
      if (kDebugMode) {
        print('Error creating file $filePath: $e');
      }
    }

    return newFile!;
  }

  Future<void> saveFile(
      File file, String path, Stream<List<int>> stream) async {
    if (kDebugMode) {
      print('Writing file $path');
    }

    int offset = 0;

    try {
      final streamReader = ChunkedStreamReader(stream);
      while (true) {
        final buffer = await streamReader.readChunk(64000);
        if (kDebugMode) {
          print('Buffer size: ${buffer.length} - offset: $offset');
        }

        if (buffer.isEmpty) {
          if (kDebugMode) {
            print('The buffer is empty; reading from the stream is done!');
          }
          break;
        }

        await file.write(offset, buffer);
        offset += buffer.length;
      }
    } catch (e) {
      if (kDebugMode) {
        print('Exception writing the file $path:\n${e.toString()}');
      }
    } finally {
      await file.close();
    }
  }

  Future<void> getFiles(String path) async {
    final dir = await Directory.read(repo, path);
    final items = dir.map((entry) => entry.name).toList();

    setState(() {
      contents.clear();
      contents.addAll(items);
    });
  }

  void showAlertDialog(BuildContext context, String path, int size) {
    Widget previewFileButton = TextButton(
      child: const Text("Preview"),
      onPressed: () async {
        Navigator.of(context).pop();
        await nativeChannels.previewOuiSyncFile(
            "org.equalitie.ouisync_example", path, size);
      },
    );
    Widget shareFileButton = TextButton(
        child: const Text("Share"),
        onPressed: () async {
          Navigator.of(context).pop();
          await nativeChannels.shareOuiSyncFile(
              "org.equalitie.ouisync_example", path, size);
        });
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
