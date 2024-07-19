import 'package:flutter/services.dart';
import '../client.dart';
import 'message_matcher.dart';

class ChannelClient extends Client {
  final MethodChannel _channel;
  bool _isClosed = false;
  final _messageMatcher = MessageMatcher();

  ChannelClient(String channelName) : _channel = MethodChannel(channelName) {
    _channel.setMethodCallHandler(_handleMethodCall);
  }

  Future<void> initialize() async {
    await _messageMatcher.sendAndAwaitResponse("", {}, (Uint8List message) async {
      await _channel.invokeMethod("initialize", message);
    });
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
  }

  Future<dynamic> _handleMethodCall(MethodCall call) async {
    final args = (call.arguments as List<Object?>).cast<int>();
    _messageMatcher.handleResponse(Uint8List.fromList(args));
    return null;
  }

  Subscriptions subscriptions() => _messageMatcher.subscriptions();

}
