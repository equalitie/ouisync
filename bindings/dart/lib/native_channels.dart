import 'dart:async';
import 'dart:collection';

import 'package:flutter/services.dart';

import 'ouisync_plugin.dart' show Session, Repository, File;

/// Enum for handling the reponse from the previewFile method
enum PreviewFileResult { previewOK, mimeTypeNull, noDefaultApp }

String previewFileResultToString(PreviewFileResult previewFileResult) {
  return switch (previewFileResult) {
    PreviewFileResult.previewOK => 'previewOK',
    PreviewFileResult.mimeTypeNull => 'mimeTypeNull',
    PreviewFileResult.noDefaultApp => 'noDefaultApp'
  };
}

PreviewFileResult? previewFileResultFromString(String previewFileResult) {
  switch (previewFileResult) {
    case 'previewOK':
      {
        return PreviewFileResult.previewOK;
      }
    case 'mimeTypeNull':
      {
        return PreviewFileResult.mimeTypeNull;
      }
    case 'noDefaultApp':
      {
        return PreviewFileResult.noDefaultApp;
      }
  }

  print('Failed to convert string "$previewFileResult" to enum');
  return null;
}

/// MethodChannel handler for calling functions
/// implemented natively, and viceversa.
class NativeChannels {
  NativeChannels(this._session) {
    _channel.setMethodCallHandler(_methodHandler);
  }

  final MethodChannel _channel = const MethodChannel('ouisync_plugin');

  // We need this session` variable to be able to close the session
  // from inside the java/kotlin code when the plugin is detached from the
  // engine. This is because when the app is set up to ignore battery
  // optimizations, Android may let the native (c/c++/rust) code running even
  // after the plugin was detached.
  final Session _session;

  Repository? _repository;
  // Cache of open files.
  final _files = FileCache();

  /// Replaces the current [repository] instance with a new one.
  ///
  /// This method is used when the user switch between repositories;
  /// the [repository] passed in to this function is used for any
  /// required operation.
  ///
  /// [repository] is the current repository in the app.
  set repository(Repository? repository) {
    for (var file in _files.removeAll()) {
      unawaited(file.close());
    }

    _repository = repository;
  }

  /// Handler method in charge of picking the right function based in the
  /// [call.method].
  ///
  /// [call] is the object sent from the native platform with the function name ([call.method])
  /// and any arguments included ([call.arguments])
  Future<dynamic> _methodHandler(MethodCall call) async {
    switch (call.method) {
      case 'openFile':
        final args = call.arguments as Map<Object?, Object?>;
        final path = args["path"] as String;

        return await _openFile(path);

      case 'readFile':
        final args = call.arguments as Map<Object?, Object?>;
        final id = args["id"] as int;
        final chunkSize = args["chunkSize"] as int;
        final offset = args["offset"] as int;

        return await _readFile(id, chunkSize, offset);

      case 'closeFile':
        final args = call.arguments as Map<Object?, Object?>;
        final id = args["id"] as int;

        return await _closeFile(id);

      case 'copyFileToRawFd':
        final args = call.arguments as Map<Object?, Object?>;
        final srcPath = args["srcPath"] as String;
        final dstFd = args["dstFd"] as int;

        return await _copyFileToRawFd(srcPath, dstFd);

      case 'stopSession':
        _session.closeSync();
        return;

      default:
        throw Exception('No method called ${call.method} was found');
    }
  }

  Future<int?> _openFile(String path) async {
    final id = _files.insert(await File.open(_repository!, path));
    print('openFile(path=$path) -> id=$id');
    return id;
  }

  Future<void> _closeFile(int id) async {
    print('closeFile(id=$id)');

    final file = _files.remove(id);

    if (file != null) {
      await file.close();
    }
  }

  Future<Uint8List> _readFile(int id, int chunkSize, int offset) async {
    print('readFile(id=$id, chunkSize=$chunkSize, offset=$offset)');

    final file = _files[id];

    if (file != null) {
      final chunk = await file.read(offset, chunkSize);
      return Uint8List.fromList(chunk);
    } else {
      throw Exception('failed to read file with id=$id: not opened');
    }
  }

  Future<void> _copyFileToRawFd(String srcPath, int dstFd) async {
    final file = await File.open(_repository!, srcPath);
    await file.copyToRawFd(dstFd);
  }

  /// Invokes the native method (In Android, it creates a share intent using the custom PipeProvider).
  ///
  /// [path] is the location of the file to share, including its full name (<path>/<file-name.ext>).
  /// [size] is the lenght of the file (bytes).
  Future<void> setIsReceivingShareIntent() async {
    final dynamic result =
        await _channel.invokeMethod('setIsReceivingShareIntent');
    print('setIsReceivingShareIntent called ($result)');
  }

  /// Invokes the native method (In Android, it creates a share intent using the custom PipeProvider).
  ///
  /// [path] is the location of the file to share, including its full name (<path>/<file-name.ext>).
  /// [size] is the lenght of the file (bytes).
  Future<void> shareOuiSyncFile(String authority, String path, int size) async {
    final dynamic result = await _channel.invokeMethod(
        'shareFile', {"authority": authority, "path": path, "size": size});
    print('shareFile result: $result');
  }

  /// Invokes the native method (In Android, it creates an intent using the custom PipeProvider).
  ///
  /// [path] is the location of the file to preview, including its full name (<path>/<file-name.ext>).
  /// [size] is the lenght of the file (bytes).
  Future<PreviewFileResult?> previewOuiSyncFile(
      String authority, String path, int size,
      {bool useDefaultApp = false}) async {
    var args = {"authority": authority, "path": path, "size": size};

    if (useDefaultApp == true) {
      args["useDefaultApp"] = true;
    }

    final dynamic result = await _channel.invokeMethod('previewFile', args);
    final previewFileResult = previewFileResultFromString(result);

    final message = switch (previewFileResult) {
      PreviewFileResult.previewOK => 'started for file $path',
      PreviewFileResult.mimeTypeNull => 'failed due to unknown file extension',
      PreviewFileResult.noDefaultApp =>
        'failed due to not app available for this file type',
      _ => 'status unknown'
    };

    print("previewFile result: View file intent $message");

    return previewFileResult;
  }
}

// Cache of open files.
class FileCache {
  final _files = HashMap<int, File>();
  var _nextId = 0;

  List<File> removeAll() {
    var files = _files.values.toList();
    _files.clear();
    return files;
  }

  int insert(File file) {
    final id = _nextId;
    _nextId += 1;
    _files[id] = file;
    return id;
  }

  File? remove(int id) => _files.remove(id);

  File? operator [](int id) => _files[id];
}
