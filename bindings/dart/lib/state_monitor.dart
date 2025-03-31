import 'package:messagepack/messagepack.dart' show Packer, Unpacker;

import 'client.dart';
import 'bindings.dart'
    show
        RequestSessionSubscribeToStateMonitor,
        RequestSessionGetStateMonitor,
        ResponseStateMonitor,
        ResponseNone,
        InvalidData;

// Used to identify child state monitors.
class MonitorId implements Comparable<MonitorId> {
  final String name;
  // This one is now shown to the user, it allows us to have multiple monitors of the same name.
  final int disambiguator;

  MonitorId(this.name, this.disambiguator);

  // For when we expect the name to uniquely identify the child.
  static MonitorId expectUnique(String name) => MonitorId(name, 0);

  static MonitorId decode(Unpacker u) {
    final raw = u.unpackString()!;

    // A string in the format "name:disambiguator".
    final colon = raw.lastIndexOf(':');
    final name = raw.substring(0, colon);
    final disambiguator = int.parse(raw.substring(colon + 1));

    return MonitorId(name, disambiguator);
  }

  void encode(Packer p) => p.packString(toString());

  @override
  String toString() {
    return "$name:$disambiguator";
  }

  @override
  int compareTo(MonitorId other) {
    // Negative return value means `this` will be appear first.
    final cmp = name.compareTo(other.name);
    if (cmp == 0) {
      return disambiguator - other.disambiguator;
    }
    return cmp;
  }
}

class StateMonitorNode {
  final Map<String, String> values;
  final List<MonitorId> children;

  StateMonitorNode(
    this.values,
    this.children,
  );

  static StateMonitorNode? decode(Unpacker u) {
    if (u.unpackListLength() != 2) {
      return null;
    }

    final values = _decodeValues(u);
    final children = _decodeChildren(u);

    return StateMonitorNode(
      values,
      children,
    );
  }

  static Map<String, String> _decodeValues(Unpacker u) {
    final n = u.unpackMapLength();
    return Map.fromEntries(Iterable.generate(n, (_) {
      final k = u.unpackString()!;
      final v = u.unpackString()!;
      return MapEntry(k, v);
    }));
  }

  static List<MonitorId> _decodeChildren(Unpacker u) {
    final n = u.unpackListLength();
    return Iterable.generate(n, (_) => MonitorId.decode(u)).toList();
  }

  int? parseIntValue(String name) {
    final str = values[name];
    if (str == null) return null;
    return int.tryParse(str);
  }

  double? parseDoubleValue(String name) {
    final str = values[name];
    if (str == null) return null;
    return double.tryParse(str);
  }

  @override
  String toString() =>
      "StateMonitorNode { values:$values, children:$children }";
}

class StateMonitor {
  final Client _client;
  final List<MonitorId> _path;

  StateMonitor._(this._client, this._path);

  static StateMonitor getRoot(Client client) =>
      StateMonitor._(client, <MonitorId>[]);

  StateMonitor child(MonitorId childId) =>
      StateMonitor._(_client, [..._path, childId]);

  /// Stream of change notifications
  Stream<void> get changes => _client
      .subscribe(RequestSessionSubscribeToStateMonitor(path: _path))
      .map((_) {});

  @override
  String toString() => "StateMonitor($_path)";

  Future<StateMonitorNode?> load() async {
    final response =
        await _client.invoke(RequestSessionGetStateMonitor(path: _path));

    switch (response) {
      case ResponseStateMonitor(value: final node):
        return node;
      case ResponseNone():
        return null;
      default:
        throw InvalidData('unexpected response');
    }
  }
}
