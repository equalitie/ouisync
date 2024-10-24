import 'dart:async';

import 'package:flutter/services.dart';
import '../client.dart';
import 'message_matcher.dart';

class ChannelClient extends Client {
  final MethodChannel _channel;
  final _messageMatcher = MessageMatcher();
  late final Future<void> initialized;
  final void Function()? _onClose;

  ChannelClient(String channelName, [this._onClose]): _channel = MethodChannel(channelName) {
    _channel.setMethodCallHandler(_handleMethodCall);
    initialized = _channel.invokeMethod("initialize");
  }

  @override
  Future<T> invoke<T>(String method, [Object? args]) async {
    return await _messageMatcher.sendAndAwaitResponse(method, args, (Uint8List message) async {
      await _channel.invokeMethod("invoke", message);
    });
  }

  @override
  Future<void> close() async {
    _channel.setMethodCallHandler(null);
    _messageMatcher.close();
    _onClose?.call();
  }

  Future<dynamic> _handleMethodCall(MethodCall call) async {
    switch (call.method) {
      case "reset":
        print("😡 Connection to Ouisync has been ${call.arguments as String}");
        unawaited(close()); // this is not actually async anymore
        break;
      case "response":
        final args = (call.arguments as List<Object?>).cast<int>();
        _messageMatcher.handleResponse(Uint8List.fromList(args));
        break;
      default:
        print("🌵 Received an unrecognized method call from Native: ${call.method}");
        break;
    }
  }

  @override
  Subscriptions subscriptions() => _messageMatcher.subscriptions();
}
