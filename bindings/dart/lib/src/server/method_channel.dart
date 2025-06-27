import 'dart:async';

import 'package:flutter/services.dart';

import '../server.dart';
import '../../generated/api.g.dart' show LogLevel;

class MethodChannelServer extends Server {
  final _channel = MethodChannel('org.equalitie.ouisync.plugin');
  Function(LogLevel, String)? _logCallback;

  MethodChannelServer({
    required super.configPath,
    super.debugLabel,
  }) {
    _channel.setMethodCallHandler(_handleMethodCall);
  }

  @override
  Future<void> initLog() => _channel.invokeMethod('initLog');

  @override
  Future<void> start() => _channel.invokeMethod<void>('start', {
        'configPath': configPath,
        'debugLabel': debugLabel,
      });

  @override
  Future<void> stop() => _channel.invokeMethod<void>('stop');

  @override
  Future<void> notify({
    String? channelName,
    String? contentTitle,
    String? contentText,
  }) =>
      _channel.invokeMethod<void>('notify', {
        'channelName': channelName,
        'contentTitle': contentTitle,
        'contentText': contentText,
      });

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
