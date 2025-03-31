import 'package:messagepack/messagepack.dart';

import 'client.dart' show Client;
import 'state_monitor.dart' show MonitorId, StateMonitorNode;

class DecodeError extends ArgumentError {
  DecodeError() : super('decode error');
}

void _encodeNullable<T>(Packer p, T? value, Function(Packer, T) encode) {
  value != null ? encode(p, value) : p.packNull();
}

void _encodeList<T>(Packer p, List<T> list, Function(Packer, T) encodeElement) {
  p.packListLength(list.length);
  for (final e in list) {
    encodeElement(p, e);
  }
}

List<T> _decodeList<T>(Unpacker u, T Function(Unpacker) decodeElement) =>
    List.generate(u.unpackListLength(), (_) => decodeElement(u));

// ignore: unused_element
void _encodeMap<K, V>(
  Packer p,
  Map<K, V> map,
  Function(Packer, K) encodeKey,
  Function(Packer, V) encodeValue,
) {
  p.packMapLength(map.length);

  for (final e in map.entries) {
    encodeKey(p, e.key);
    encodeValue(p, e.value);
  }
}

Map<K, V> _decodeMap<K, V>(
  Unpacker u,
  K Function(Unpacker) decodeKey,
  V Function(Unpacker) decodeValue,
) => Map.fromEntries(
  Iterable.generate(u.unpackMapLength(), (_) {
    final k = decodeKey(u);
    final v = decodeValue(u);
    return MapEntry(k, v);
  }),
);

void _encodeDateTime(Packer p, DateTime v) =>
    p.packInt(v.millisecondsSinceEpoch);

DateTime? _decodeDateTime(Unpacker u) {
  final n = u.unpackInt();
  return n != null ? DateTime.fromMillisecondsSinceEpoch(n) : null;
}

void _encodeDuration(Packer p, Duration v) => p.packInt(v.inMilliseconds);

Duration? _decodeDuration(Unpacker u) {
  final n = u.unpackInt();
  return n != null ? Duration(milliseconds: n) : null;
}
