/* TODO: do we still need this?
import 'dart:async';

import 'package:flutter/services.dart';
import '../errors.dart' show mapError;
import '../client.dart';
import 'message_matcher.dart';

class ChannelClient extends Client {
  final MethodChannel _channel;
  final _messageMatcher = MessageMatcher();
  late final Future<void> initialized;
  final void Function()? _onClose;

  ChannelClient(String channelName, [this._onClose])
      : _channel = MethodChannel(channelName) {
    _channel.setMethodCallHandler(_handleMethodCall);
    initialized = _channel
        .invokeMethod("initialize")
        .onError((PlatformException err, _) => throw mapError(err));
  }

  @override
  Future<T> invoke<T>(String method, [Object? args]) async {
    final (message, response) = _messageMatcher.send(method, args);
    await _channel
        .invokeMethod("invoke", message)
        .onError((PlatformException err, _) => throw mapError(err));

    return await response;
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
        _messageMatcher.receive(Uint8List.fromList(args));
        break;
      default:
        print(
            "🌵 Received an unrecognized method call from host: ${call.method}");
        break;
    }
  }

  @override
  void subscribe(int id, StreamSink<Object?> sink) =>
      _messageMatcher.subscribe(id, sink);

  @override
  void unsubscribe(int id) => _messageMatcher.unsubscribe(id);
}
*/
