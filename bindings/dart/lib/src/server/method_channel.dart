import 'dart:async';

import 'package:flutter/services.dart';

import '../server.dart';
import '../../generated/api.g.dart' show LogLevel;

class MethodChannelServer extends Server {
  final _channel = MethodChannel('org.equalitie.ouisync.plugin');
  Function(LogLevel, String)? _logCallback;

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
  Future<void> start() => _channel.invokeMethod<int>('start', {
        'configPath': configPath,
        'debugLabel': debugLabel,
      });

  @override
  Future<void> stop() => _channel.invokeMethod<int>('stop');

  Future<void> _handleMethodCall(MethodCall call) {
    switch (call.method) {
      case 'log':
        final logCallback = _logCallback;
        if (logCallback != null) {
          final args = call.arguments as Map<Object?, Object?>;
          final level = LogLevel.fromInt(args['level'] as int)!;
          final message = args['message'] as String;

          logCallback(level, message);
        }
    }

    return Future.value();
  }
}
