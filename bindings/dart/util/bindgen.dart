import 'dart:io';

// Generate 'lib/bindings.g.dart' by invoking the 'ouisync-bindgen' tool with the right arguments.

Future<void> main() async {
  final result = await Process.run(
    'cargo',
    [
      'run',
      '--package',
      'ouisync-bindgen',
      '--',
      '--language',
      'dart',
    ],
    workingDirectory: '../..',
  );

  await File('lib/bindings.g.dart').writeAsString(result.stdout);
}
