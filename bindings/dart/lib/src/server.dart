import 'dart:io';

import 'server/ffi.dart';
import 'server/method_channel.dart';

/// Handle to a Ouisync service.
abstract class Server {
  final String configPath;
  final String? debugLabel;

  Server({required this.configPath, this.debugLabel});

  factory Server.create({required String configPath, String? debugLabel}) {
    if (Platform.isAndroid) {
      return MethodChannelServer(
        configPath: configPath,
        debugLabel: debugLabel,
      );
    } else {
      return FfiServer(
        configPath: configPath,
        debugLabel: debugLabel,
      );
    }
  }

  /// Initializes logging in the underlying Ouisync service.
  Future<void> initLog();

  /// Starts the server. After this function completes the server is ready to accept client
  /// connections.
  Future<void> start();

  /// Stops the server.
  Future<void> stop();

  /// Setup notification indicating the server is running. Currently this has effect only on
  /// Android (where the server is backed by [foreground service]
  /// (https://developer.android.com/develop/background-work/services/fgs)).
  Future<void> notify({
    String? channelName,
    String? contentTitle,
    String? contentText,
  }) =>
      Future.value();
}
