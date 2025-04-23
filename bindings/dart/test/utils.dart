import 'dart:io';

/// Same as `Directory.delete` but ignores some irrelevant exceptions. Those exceptions are
/// sometimes throws on the windows CI but they don't affect the tests (we are only cleaning up the
/// temp dirs during test teardown) so they should be safe to ignore.
Future<void> deleteTempDir(Directory dir) async {
  try {
    await dir.delete(recursive: true);
  } on PathAccessException catch (e) {
    stderr.write('failed to delete ${dir.path}: $e');
  } on PathNotFoundException catch (e) {
    stderr.write('failed to delete ${dir.path}: $e');
  }
}
