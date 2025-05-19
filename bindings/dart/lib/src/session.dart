import '../generated/api.g.dart' as api show Session;
import '../generated/api.g.dart' hide Session;
import 'client.dart';
import 'server.dart';
import 'state_monitor.dart';

class Session extends api.Session {
  final Server? _server;

  Session._(super.client, this._server);

  /// Creates a new session in this process.
  /// [configPath] is a path to a directory where configuration files shall be stored. If it
  /// doesn't exists, it will be created.
  static Future<Session> create({
    required String configPath,
    String? debugLabel,
  }) async {
    Server? server;

    // Try to start our own server but if one is already running connect to
    // that one instead.
    try {
      server = await Server.start(
        configPath: configPath,
        debugLabel: debugLabel,
      );
    } on ServiceAlreadyRunning catch (_) {}

    final client = await Client.connect(configPath: configPath);

    return Session._(client, server);
  }

  /// Stream of network events
  Stream<NetworkEvent> get networkEvents =>
      client.subscribe(RequestSessionSubscribeToNetwork()).map(
            (r) => switch (r) {
              ResponseNetworkEvent(value: final value) => value,
              _ => throw InvalidData('unexpected event type'),
            },
          );

  StateMonitor get rootStateMonitor => StateMonitor.getRoot(client);

  /// Try to gracefully close connections to peers then close the session.
  Future<void> close() async {
    await client.close();
    await _server?.stop();
  }
}

extension RepositoryExtension on Repository {
  /// Stream of repository notification events
  Stream<void> get events =>
      client.subscribe(RequestRepositorySubscribe(repo: handle)).map(
            (r) => switch (r) {
              ResponseRepositoryEvent() => null,
              _ => throw InvalidData('unexpected event type'),
            },
          );

  Future<StateMonitor> get stateMonitor async {
    final path = await getPath();
    return StateMonitor.getRoot(client)
        .child(MonitorId.expectUnique('Repositories'))
        .child(MonitorId.expectUnique(path));
  }
}

extension SetLocalSecretExtension on SetLocalSecret {
  LocalSecret toLocalSecret() => switch (this) {
        SetLocalSecretPassword(value: final password) => LocalSecretPassword(
            password,
          ),
        SetLocalSecretKeyAndSalt(key: final key) => LocalSecretSecretKey(key),
      };
}
