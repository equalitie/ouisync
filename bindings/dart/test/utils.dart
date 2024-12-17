import 'dart:io';
import 'dart:math';

/// Get unique socket path for `Client` and/or `Server`.
String getTestSocketPath(String parent) {
  final suffix = _randomString(8);

  if (Platform.isWindows) {
    return '\\\\.\\pipe\\$suffix';
  } else {
    return '$parent/$suffix.sock';
  }
}

String _randomString(int length) {
  const String charset = 'abcdefghijklmnopqrstuvwxyz';
  final random = Random();
  final buffer = StringBuffer();

  for (var i = 0; i < length; i++) {
    buffer.write(charset[random.nextInt(charset.length)]);
  }

  return buffer.toString();
}
