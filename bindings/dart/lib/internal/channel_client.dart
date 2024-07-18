import 'package:flutter/services.dart';
import '../client.dart';

class ChannelClient extends Client {
    final MethodChannel _channel;
    final _subscriptions = Subscriptions();

    ChannelClient(String channelName) : _channel = MethodChannel(channelName) {
        _channel.setMethodCallHandler(_handleMethodCall);
    }

    @override
    Future<T> invoke<T>(String method, [Object? args]) async {
        return await _channel.invokeMethod(method, args);
    }

    @override
    Future<void> close() async {
        _channel.setMethodCallHandler(null);
    }

    Future<dynamic> _handleMethodCall(MethodCall call) async {
        if (call.method != "notify") {
            throw PlatformException(code: "error", message: "Unsupported method: ${call.method}");
        }

        final args = call.arguments;

        if (args !is List<Object>) {
            throw PlatformException(code: "error", message: "Invalid arguments");
        }

        if (args[0] !is int) {
            throw PlatformException(code: "error", message: "First argument must be subsciption ID");
        }

        final subscriptionId = args[0];

        if (args[1] !is Object?) {
            throw PlatformException(code: "error", message: "Second argument must be of type Object?");
        }

        final payload = args[1];

        _subscriptions.handle(subscriptionId, payload);

        return null;
    }

    Subscriptions subscriptions() => _subscriptions;
}
