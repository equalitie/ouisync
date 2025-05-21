import 'dart:async';

import 'package:flutter/services.dart';

import '../server.dart';
import '../../generated/api.g.dart' show LogLevel;

class MethodChannelServer extends Server {
  final _channel = MethodChannel('org.equalitie.ouisync.plugin');
  Function(LogLevel, String)? _logCallback;
  int _handle = 0;
  // Prevents calling start/stop concurrently.
  Completer<void> _lock = Completer()..complete();

  MethodChannelServer({required super.configPath, super.debugLabel}) {
    _channel.setMethodCallHandler(_handleMethodCall);
  }

  @override
  void initLog({
    bool stdout = false,
    String? file,
    Function(LogLevel, String)? callback,
  }) {
    _logCallback = callback;
    _channel.invokeMethod('initLog', {'stdout': stdout, 'file': file});
  }

  @override
  Future<void> start() async {
    await _lock.future;

    if (_handle != 0) {
      return;
    }

    _lock = Completer();

    try {
      _handle = await _channel.invokeMethod<int>('start', {
            'configPath': configPath,
            'debugLabel': debugLabel,
          }) ??
          0;
    } finally {
      _lock.complete();
    }
  }

  @override
  Future<void> stop() async {
    await _lock.future;

    if (_handle == 0) {
      return;
    }

    _lock = Completer();

    try {
      await _channel.invokeMethod<int>('stop', {'handle': _handle});
      _handle = 0;
    } finally {
      _lock.complete();
    }
  }

  Future<void> _handleMethodCall(MethodCall call) {
    switch (call.method) {
      case 'log':
        final logCallback = _logCallback;
        if (logCallback != null) {
          final args = call.arguments as Map<String, Object?>;
          final level = LogLevel.fromInt(args['level'] as int)!;
          final message = args['message'] as String;

          logCallback(level, message);
        }
    }

    return Future.value();
  }
}
