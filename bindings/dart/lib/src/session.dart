import '../generated/api.g.dart' as api show Session;
import '../generated/api.g.dart' hide Session;
import 'client.dart';
import 'state_monitor.dart';

class Session extends api.Session {
  Session._(super.client);

  /// Creates a new session in this process.
  /// [configPath] is a path to a directory where configuration files shall be stored. If it
  /// doesn't exists, it will be created.
  static Future<Session> create({
    required String configPath,
  }) =>
      Client.connect(configPath: configPath).then(Session._);

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
  Future<void> close() => client.close();
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
