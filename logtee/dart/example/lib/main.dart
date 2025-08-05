import 'dart:io';

import 'package:flutter/material.dart';

import 'package:logtee/logtee.dart';
import 'package:path/path.dart';

Future<void> main() async {
  final tempDir = await Directory.systemTemp.createTemp();
  final logPath = join(tempDir.path, 'log.txt');

  runApp(MyApp(logPath: logPath));
}

class MyApp extends StatefulWidget {
  final String logPath;

  const MyApp({required this.logPath, super.key});

  @override
  State<MyApp> createState() => _MyAppState();
}

class _MyAppState extends State<MyApp> {
  Logtee? logtee;
  int count = 1;

  @override
  void dispose() {
    logtee?.stop();
    logtee = null;

    super.dispose();
  }

  @override
  Widget build(BuildContext context) => MaterialApp(
    home: Scaffold(
      appBar: AppBar(title: const Text('Logtee example app')),
      body: Padding(
        padding: EdgeInsets.all(8.0),
        child: Builder(
          builder:
              (context) => Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                spacing: 8.0,
                children: [
                  SwitchListTile(
                    title: const Text('Enable logtee'),
                    value: logtee != null,
                    onChanged: (bool value) {
                      if (value) {
                        setState(() {
                          logtee ??= Logtee.start(widget.logPath);
                        });
                      } else {
                        setState(() {
                          logtee?.stop();
                          logtee = null;
                        });
                      }
                    },
                  ),
                  ListTile(
                    title: const Text('View log file'),
                    onTap: () => _viewLogFile(context),
                  ),
                  ListTile(
                    title: const Text('Emit log message'),
                    onTap: () {
                      // ignore: avoid_print
                      print('Hello from Logtee example app! ($count)');
                      count++;
                    },
                  ),
                ],
              ),
        ),
      ),
    ),
  );

  Future<void> _viewLogFile(BuildContext context) async {
    final file = File(widget.logPath);
    final content = await file.exists() ? await file.readAsString() : '';

    if (!context.mounted) {
      return;
    }

    await Navigator.push(
      context,
      MaterialPageRoute<void>(
        builder:
            (BuildContext context) => Scaffold(
              appBar: AppBar(title: Text('Log file: ${widget.logPath}')),
              body: Container(
                padding: EdgeInsets.all(8.0),
                child: Text(content),
              ),
            ),
      ),
    );
  }
}
