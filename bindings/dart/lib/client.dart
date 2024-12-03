import 'dart:async';

export 'internal/channel_client.dart';
export 'internal/socket_client.dart';

abstract class Client {
  Future<T> invoke<T>(String method, [Object? args]);
  Stream<T> subscribe<T>(String method, Object? arg);
  Future<void> close();
}
