import 'package:flutter/services.dart';
import '../client.dart';
import 'message_matcher.dart';

class ChannelClient extends Client {
  final MethodChannel _channel;
  final _messageMatcher = MessageMatcher();
  final void Function()? _onConnectionReset;

  ChannelClient(String channelName,
                void Function()? this._onConnectionReset) : _channel = MethodChannel(channelName) {
    _channel.setMethodCallHandler(_handleMethodCall);
  }

  Future<void> initialize() async {
    await _channel.invokeMethod("initialize");
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
    switch (call.method) {
      case "reset":
        print("😡 Connection to File Provider has been ${call.arguments as String}");
        _onConnectionReset?.call();
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
