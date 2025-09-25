import 'dart:io';

// Generate 'lib/generated/api.g.dart' by invoking the 'ouisync-bindgen' tool with the right arguments.

Future<void> main() async {
  final result = await Process.run('cargo', [
    'run',
    '--package',
    'ouisync-bindgen',
    '--',
    'dart',
  ], workingDirectory: '../..');

  if (result.exitCode != 0) {
    stderr.write(result.stderr);
    exit(result.exitCode);
  }

  await Directory('lib/generated').create(recursive: true);
  await File('lib/generated/api.g.dart').writeAsString(result.stdout);
}
