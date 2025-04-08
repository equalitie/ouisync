import 'package:messagepack/messagepack.dart';

import 'client.dart' show Client;
import 'state_monitor.dart' show MonitorId, StateMonitorNode;

class DecodeError extends ArgumentError {
  DecodeError() : super('decode error');
}

class UnexpectedResponse extends InvalidData {
  UnexpectedResponse() : super('unexpected response');
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

/// Wrapper for `List<T>` which provides value-based equality and hash code.
final class _ListWrapper<T> {
  final List<T> list;

  _ListWrapper(this.list);

  @override
  bool operator ==(Object other) =>
      other is _ListWrapper<T> &&
      list.length == other.list.length &&
      list.indexed.every((p) => p.$2 == other.list[p.$1]);

  @override
  int get hashCode => Object.hashAll(list);
}

/// Symmetric encryption/decryption secret key.
///
/// Note: this implementation tries to prevent certain types of attacks by making sure the
/// underlying sensitive key material is always stored at most in one place. This is achieved by
/// putting it on the heap which means it is not moved when the key itself is moved which could
/// otherwise leave a copy of the data in memory. Additionally, the data is behind a `Arc` which
/// means the key can be cheaply cloned without actually cloning the data. Finally, the data is
/// scrambled (overwritten with zeros) when the key is dropped to make sure it does not stay in
/// the memory past its lifetime.
final class SecretKey {
  final List<int> value;

  SecretKey(this.value);

  void encode(Packer p) {
    p.packBinary(value);
  }

  static SecretKey? decode(Unpacker u) {
    return SecretKey(u.unpackBinary());
  }

  @override
  operator==(Object other) =>
      other is SecretKey &&
      _ListWrapper(other.value) == _ListWrapper(value);

  @override
  int get hashCode => _ListWrapper(value).hashCode;
}

/// A simple wrapper over String to avoid certain kinds of attack. For more elaboration please see
/// the documentation for the SecretKey structure.
final class Password {
  final String value;

  Password(this.value);

  void encode(Packer p) {
    p.packString(value);
  }

  static Password? decode(Unpacker u) {
    final value = u.unpackString();
    return value != null ? Password(value) : null;
  }

  @override
  operator==(Object other) =>
      other is Password &&
      other.value == value;

  @override
  int get hashCode => value.hashCode;

  @override
  String toString() => '****';
}

final class PasswordSalt {
  final List<int> value;

  PasswordSalt(this.value);

  void encode(Packer p) {
    p.packBinary(value);
  }

  static PasswordSalt? decode(Unpacker u) {
    return PasswordSalt(u.unpackBinary());
  }

  @override
  operator==(Object other) =>
      other is PasswordSalt &&
      _ListWrapper(other.value) == _ListWrapper(value);

  @override
  int get hashCode => _ListWrapper(value).hashCode;
}

/// Strongly typed storage size.
final class StorageSize {
  final int bytes;

  StorageSize({
    required this.bytes,
  });

  void encode(Packer p) {
    p.packListLength(1);
    p.packInt(bytes);
  }

  static StorageSize? decode(Unpacker u) {
    switch (u.unpackListLength()) {
      case 0: return null;
      case 1: break;
      default: throw DecodeError();
    }
    return StorageSize(
      bytes: (u.unpackInt())!,
    );
  }

  @override
  operator==(Object other) =>
      other is StorageSize &&
      other.bytes == bytes;

  @override
  int get hashCode => bytes.hashCode;
}

/// Access mode of a repository.
enum AccessMode {
  /// Repository is neither readable not writtable (but can still be synced).
  blind,
  /// Repository is readable but not writtable.
  read,
  /// Repository is both readable and writable.
  write,
  ;

  static AccessMode? fromInt(int n) {
    switch (n) {
      case 0: return AccessMode.blind;
      case 1: return AccessMode.read;
      case 2: return AccessMode.write;
      default: return null;
    }
  }

  int toInt() => switch (this) {
      AccessMode.blind => 0,
      AccessMode.read => 1,
      AccessMode.write => 2,
    };

  static AccessMode? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    p.packInt(toInt());
  }

}

sealed class LocalSecret {
  void encode(Packer p) {
    switch (this) {
      case LocalSecretPassword(
        value: final value,
      ):
        p.packMapLength(1);
        p.packString('Password');
        value.encode(p);
      case LocalSecretSecretKey(
        value: final value,
      ):
        p.packMapLength(1);
        p.packString('SecretKey');
        value.encode(p);
    }
  }

  static LocalSecret? decode(Unpacker u) {
    try {
      switch (u.unpackString()) {
        case null: return null;
        default: throw DecodeError();
      }
    } on FormatException {
      if (u.unpackMapLength() != 1) throw DecodeError();
      switch (u.unpackString()) {
        case "Password":
          return LocalSecretPassword(
            (Password.decode(u))!,
          );
        case "SecretKey":
          return LocalSecretSecretKey(
            (SecretKey.decode(u))!,
          );
        case null: return null;
        default: throw DecodeError();
      }
    }
  }

}

class LocalSecretPassword extends LocalSecret {
  final Password value;

  LocalSecretPassword(this.value);
}

class LocalSecretSecretKey extends LocalSecret {
  final SecretKey value;

  LocalSecretSecretKey(this.value);
}

sealed class SetLocalSecret {
  void encode(Packer p) {
    switch (this) {
      case SetLocalSecretPassword(
        value: final value,
      ):
        p.packMapLength(1);
        p.packString('Password');
        value.encode(p);
      case SetLocalSecretKeyAndSalt(
        key: final key,
        salt: final salt,
      ):
        p.packMapLength(1);
        p.packString('KeyAndSalt');
        p.packListLength(2);
        key.encode(p);
        salt.encode(p);
    }
  }

  static SetLocalSecret? decode(Unpacker u) {
    try {
      switch (u.unpackString()) {
        case null: return null;
        default: throw DecodeError();
      }
    } on FormatException {
      if (u.unpackMapLength() != 1) throw DecodeError();
      switch (u.unpackString()) {
        case "Password":
          return SetLocalSecretPassword(
            (Password.decode(u))!,
          );
        case "KeyAndSalt":
          if (u.unpackListLength() != 2) throw DecodeError();
          return SetLocalSecretKeyAndSalt(
            key: (SecretKey.decode(u))!,
            salt: (PasswordSalt.decode(u))!,
          );
        case null: return null;
        default: throw DecodeError();
      }
    }
  }

}

class SetLocalSecretPassword extends SetLocalSecret {
  final Password value;

  SetLocalSecretPassword(this.value);
}

class SetLocalSecretKeyAndSalt extends SetLocalSecret {
  final SecretKey key;
  final PasswordSalt salt;

  SetLocalSecretKeyAndSalt({
    required this.key,
    required this.salt,
  });
}

/// Token to share a repository which can be encoded as a URL-formatted string and transmitted to
/// other replicas.
final class ShareToken {
  final String value;

  ShareToken(this.value);

  void encode(Packer p) {
    p.packString(value);
  }

  static ShareToken? decode(Unpacker u) {
    final value = u.unpackString();
    return value != null ? ShareToken(value) : null;
  }

  @override
  operator==(Object other) =>
      other is ShareToken &&
      other.value == value;

  @override
  int get hashCode => value.hashCode;

  @override
  String toString() => value;
}

sealed class AccessChange {
  void encode(Packer p) {
    switch (this) {
      case AccessChangeEnable(
        value: final value,
      ):
        p.packMapLength(1);
        p.packString('Enable');
        _encodeNullable(p, value, (p, e) => e.encode(p));
      case AccessChangeDisable(
      ):
        p.packString('Disable');
    }
  }

  static AccessChange? decode(Unpacker u) {
    try {
      switch (u.unpackString()) {
        case "Disable": return AccessChangeDisable();
        case null: return null;
        default: throw DecodeError();
      }
    } on FormatException {
      if (u.unpackMapLength() != 1) throw DecodeError();
      switch (u.unpackString()) {
        case "Enable":
          return AccessChangeEnable(
            SetLocalSecret.decode(u),
          );
        case null: return null;
        default: throw DecodeError();
      }
    }
  }

}

class AccessChangeEnable extends AccessChange {
  final SetLocalSecret? value;

  AccessChangeEnable(this.value);
}

class AccessChangeDisable extends AccessChange {
  AccessChangeDisable();
}

/// Type of filesystem entry.
enum EntryType {
  file,
  directory,
  ;

  static EntryType? fromInt(int n) {
    switch (n) {
      case 1: return EntryType.file;
      case 2: return EntryType.directory;
      default: return null;
    }
  }

  int toInt() => switch (this) {
      EntryType.file => 1,
      EntryType.directory => 2,
    };

  static EntryType? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    p.packInt(toInt());
  }

}

/// Network notification event.
enum NetworkEvent {
  /// A peer has appeared with higher protocol version than us. Probably means we are using
  /// outdated library. This event can be used to notify the user that they should update the app.
  protocolVersionMismatch,
  /// The set of known peers has changed (e.g., a new peer has been discovered)
  peerSetChange,
  ;

  static NetworkEvent? fromInt(int n) {
    switch (n) {
      case 0: return NetworkEvent.protocolVersionMismatch;
      case 1: return NetworkEvent.peerSetChange;
      default: return null;
    }
  }

  int toInt() => switch (this) {
      NetworkEvent.protocolVersionMismatch => 0,
      NetworkEvent.peerSetChange => 1,
    };

  static NetworkEvent? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    p.packInt(toInt());
  }

}

/// Information about a peer.
final class PeerInfo {
  final String addr;
  final PeerSource source;
  final PeerState state;
  final Stats stats;

  PeerInfo({
    required this.addr,
    required this.source,
    required this.state,
    required this.stats,
  });

  void encode(Packer p) {
    p.packListLength(4);
    p.packString(addr);
    source.encode(p);
    state.encode(p);
    stats.encode(p);
  }

  static PeerInfo? decode(Unpacker u) {
    switch (u.unpackListLength()) {
      case 0: return null;
      case 4: break;
      default: throw DecodeError();
    }
    return PeerInfo(
      addr: (u.unpackString())!,
      source: (PeerSource.decode(u))!,
      state: (PeerState.decode(u))!,
      stats: (Stats.decode(u))!,
    );
  }

  @override
  operator==(Object other) =>
      other is PeerInfo &&
      other.addr == addr &&
      other.source == source &&
      other.state == state &&
      other.stats == stats;

  @override
  int get hashCode => Object.hash(
        addr,
        source,
        state,
        stats,
      );
}

/// How was the peer discovered.
enum PeerSource {
  /// Explicitly added by the user.
  userProvided,
  /// Peer connected to us.
  listener,
  /// Discovered on the Local Discovery.
  localDiscovery,
  /// Discovered on the DHT.
  dht,
  /// Discovered on the Peer Exchange.
  peerExchange,
  ;

  static PeerSource? fromInt(int n) {
    switch (n) {
      case 0: return PeerSource.userProvided;
      case 1: return PeerSource.listener;
      case 2: return PeerSource.localDiscovery;
      case 3: return PeerSource.dht;
      case 4: return PeerSource.peerExchange;
      default: return null;
    }
  }

  int toInt() => switch (this) {
      PeerSource.userProvided => 0,
      PeerSource.listener => 1,
      PeerSource.localDiscovery => 2,
      PeerSource.dht => 3,
      PeerSource.peerExchange => 4,
    };

  static PeerSource? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    p.packInt(toInt());
  }

}

sealed class PeerState {
  void encode(Packer p) {
    switch (this) {
      case PeerStateKnown(
      ):
        p.packString('Known');
      case PeerStateConnecting(
      ):
        p.packString('Connecting');
      case PeerStateHandshaking(
      ):
        p.packString('Handshaking');
      case PeerStateActive(
        id: final id,
        since: final since,
      ):
        p.packMapLength(1);
        p.packString('Active');
        p.packListLength(2);
        id.encode(p);
        _encodeDateTime(p, since);
    }
  }

  static PeerState? decode(Unpacker u) {
    try {
      switch (u.unpackString()) {
        case "Known": return PeerStateKnown();
        case "Connecting": return PeerStateConnecting();
        case "Handshaking": return PeerStateHandshaking();
        case null: return null;
        default: throw DecodeError();
      }
    } on FormatException {
      if (u.unpackMapLength() != 1) throw DecodeError();
      switch (u.unpackString()) {
        case "Active":
          if (u.unpackListLength() != 2) throw DecodeError();
          return PeerStateActive(
            id: (PublicRuntimeId.decode(u))!,
            since: (_decodeDateTime(u))!,
          );
        case null: return null;
        default: throw DecodeError();
      }
    }
  }

}

class PeerStateKnown extends PeerState {
  PeerStateKnown();
}

class PeerStateConnecting extends PeerState {
  PeerStateConnecting();
}

class PeerStateHandshaking extends PeerState {
  PeerStateHandshaking();
}

class PeerStateActive extends PeerState {
  final PublicRuntimeId id;
  final DateTime since;

  PeerStateActive({
    required this.id,
    required this.since,
  });
}

final class PublicRuntimeId {
  final List<int> value;

  PublicRuntimeId(this.value);

  void encode(Packer p) {
    p.packBinary(value);
  }

  static PublicRuntimeId? decode(Unpacker u) {
    return PublicRuntimeId(u.unpackBinary());
  }

  @override
  operator==(Object other) =>
      other is PublicRuntimeId &&
      _ListWrapper(other.value) == _ListWrapper(value);

  @override
  int get hashCode => _ListWrapper(value).hashCode;
}

/// Network traffic statistics.
final class Stats {
  /// Total number of bytes sent.
  final int bytesTx;
  /// Total number of bytes received.
  final int bytesRx;
  /// Current send throughput in bytes per second.
  final int throughputTx;
  /// Current receive throughput in bytes per second.
  final int throughputRx;

  Stats({
    required this.bytesTx,
    required this.bytesRx,
    required this.throughputTx,
    required this.throughputRx,
  });

  void encode(Packer p) {
    p.packListLength(4);
    p.packInt(bytesTx);
    p.packInt(bytesRx);
    p.packInt(throughputTx);
    p.packInt(throughputRx);
  }

  static Stats? decode(Unpacker u) {
    switch (u.unpackListLength()) {
      case 0: return null;
      case 4: break;
      default: throw DecodeError();
    }
    return Stats(
      bytesTx: (u.unpackInt())!,
      bytesRx: (u.unpackInt())!,
      throughputTx: (u.unpackInt())!,
      throughputRx: (u.unpackInt())!,
    );
  }

  @override
  operator==(Object other) =>
      other is Stats &&
      other.bytesTx == bytesTx &&
      other.bytesRx == bytesRx &&
      other.throughputTx == throughputTx &&
      other.throughputRx == throughputRx;

  @override
  int get hashCode => Object.hash(
        bytesTx,
        bytesRx,
        throughputTx,
        throughputRx,
      );
}

/// Progress of a task.
final class Progress {
  final int value;
  final int total;

  Progress({
    required this.value,
    required this.total,
  });

  void encode(Packer p) {
    p.packListLength(2);
    p.packInt(value);
    p.packInt(total);
  }

  static Progress? decode(Unpacker u) {
    switch (u.unpackListLength()) {
      case 0: return null;
      case 2: break;
      default: throw DecodeError();
    }
    return Progress(
      value: (u.unpackInt())!,
      total: (u.unpackInt())!,
    );
  }

  @override
  operator==(Object other) =>
      other is Progress &&
      other.value == value &&
      other.total == total;

  @override
  int get hashCode => Object.hash(
        value,
        total,
      );
}

enum NatBehavior {
  endpointIndependent,
  addressDependent,
  addressAndPortDependent,
  ;

  static NatBehavior? fromInt(int n) {
    switch (n) {
      case 0: return NatBehavior.endpointIndependent;
      case 1: return NatBehavior.addressDependent;
      case 2: return NatBehavior.addressAndPortDependent;
      default: return null;
    }
  }

  int toInt() => switch (this) {
      NatBehavior.endpointIndependent => 0,
      NatBehavior.addressDependent => 1,
      NatBehavior.addressAndPortDependent => 2,
    };

  static NatBehavior? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    p.packInt(toInt());
  }

}

enum ErrorCode {
  /// No error
  ok,
  /// Insuficient permission to perform the intended operation
  permissionDenied,
  /// Invalid input parameter
  invalidInput,
  /// Invalid data (e.g., malformed incoming message, config file, etc...)
  invalidData,
  /// Entry already exists
  alreadyExists,
  /// Entry not found
  notFound,
  /// Multiple matching entries found
  ambiguous,
  /// The indended operation is not supported
  unsupported,
  /// Failed to establish connection to the server
  connectionRefused,
  /// Connection aborted by the server
  connectionAborted,
  /// Failed to send or receive message
  transportError,
  /// Listener failed to bind to the specified address
  listenerBindError,
  /// Listener failed to accept client connection
  listenerAcceptError,
  /// Operation on the internal repository store failed
  storeError,
  /// Entry was expected to not be a directory but it is
  isDirectory,
  /// Entry was expected to be a directory but it isn't
  notDirectory,
  /// Directory was expected to be empty but it isn't
  directoryNotEmpty,
  /// File or directory is busy
  resourceBusy,
  /// Failed to initialize runtime
  runtimeInitializeError,
  /// Failed to initialize logger
  loggerInitializeError,
  /// Failed to read from or write into the config file
  configError,
  /// TLS certificated not found
  tlsCertificatesNotFound,
  /// TLS certificates failed to load
  tlsCertificatesInvalid,
  /// TLS keys not found
  tlsKeysNotFound,
  /// Failed to create TLS config
  tlsConfigError,
  /// Failed to install virtual filesystem driver
  vfsDriverInstallError,
  /// Unspecified virtual filesystem error
  vfsOtherError,
  /// Another instance of the service is already running
  serviceAlreadyRunning,
  /// Store directory is not specified
  storeDirUnspecified,
  /// Unspecified error
  other,
  ;

  static ErrorCode? fromInt(int n) {
    switch (n) {
      case 0: return ErrorCode.ok;
      case 1: return ErrorCode.permissionDenied;
      case 2: return ErrorCode.invalidInput;
      case 3: return ErrorCode.invalidData;
      case 4: return ErrorCode.alreadyExists;
      case 5: return ErrorCode.notFound;
      case 6: return ErrorCode.ambiguous;
      case 8: return ErrorCode.unsupported;
      case 1025: return ErrorCode.connectionRefused;
      case 1026: return ErrorCode.connectionAborted;
      case 1027: return ErrorCode.transportError;
      case 1028: return ErrorCode.listenerBindError;
      case 1029: return ErrorCode.listenerAcceptError;
      case 2049: return ErrorCode.storeError;
      case 2050: return ErrorCode.isDirectory;
      case 2051: return ErrorCode.notDirectory;
      case 2052: return ErrorCode.directoryNotEmpty;
      case 2053: return ErrorCode.resourceBusy;
      case 4097: return ErrorCode.runtimeInitializeError;
      case 4098: return ErrorCode.loggerInitializeError;
      case 4099: return ErrorCode.configError;
      case 4100: return ErrorCode.tlsCertificatesNotFound;
      case 4101: return ErrorCode.tlsCertificatesInvalid;
      case 4102: return ErrorCode.tlsKeysNotFound;
      case 4103: return ErrorCode.tlsConfigError;
      case 4104: return ErrorCode.vfsDriverInstallError;
      case 4105: return ErrorCode.vfsOtherError;
      case 4106: return ErrorCode.serviceAlreadyRunning;
      case 4107: return ErrorCode.storeDirUnspecified;
      case 65535: return ErrorCode.other;
      default: return null;
    }
  }

  int toInt() => switch (this) {
      ErrorCode.ok => 0,
      ErrorCode.permissionDenied => 1,
      ErrorCode.invalidInput => 2,
      ErrorCode.invalidData => 3,
      ErrorCode.alreadyExists => 4,
      ErrorCode.notFound => 5,
      ErrorCode.ambiguous => 6,
      ErrorCode.unsupported => 8,
      ErrorCode.connectionRefused => 1025,
      ErrorCode.connectionAborted => 1026,
      ErrorCode.transportError => 1027,
      ErrorCode.listenerBindError => 1028,
      ErrorCode.listenerAcceptError => 1029,
      ErrorCode.storeError => 2049,
      ErrorCode.isDirectory => 2050,
      ErrorCode.notDirectory => 2051,
      ErrorCode.directoryNotEmpty => 2052,
      ErrorCode.resourceBusy => 2053,
      ErrorCode.runtimeInitializeError => 4097,
      ErrorCode.loggerInitializeError => 4098,
      ErrorCode.configError => 4099,
      ErrorCode.tlsCertificatesNotFound => 4100,
      ErrorCode.tlsCertificatesInvalid => 4101,
      ErrorCode.tlsKeysNotFound => 4102,
      ErrorCode.tlsConfigError => 4103,
      ErrorCode.vfsDriverInstallError => 4104,
      ErrorCode.vfsOtherError => 4105,
      ErrorCode.serviceAlreadyRunning => 4106,
      ErrorCode.storeDirUnspecified => 4107,
      ErrorCode.other => 65535,
    };

  static ErrorCode? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    p.packInt(toInt());
  }

}

class OuisyncException implements Exception {
  final ErrorCode code;
  final String message;
  final List<String> sources;

  OuisyncException._(this.code, String? message, this.sources)
      : message = message ?? code.toString() {
    assert(code != ErrorCode.ok);
  }

  factory OuisyncException(
    ErrorCode code, [
    String? message,
    List<String> sources = const [],
  ]) =>
    switch (code) {
      ErrorCode.ok => OuisyncException._(code, message, sources),
      ErrorCode.permissionDenied => PermissionDenied(message, sources),
      ErrorCode.invalidInput => InvalidInput(message, sources),
      ErrorCode.invalidData => InvalidData(message, sources),
      ErrorCode.alreadyExists => AlreadyExists(message, sources),
      ErrorCode.notFound => NotFound(message, sources),
      ErrorCode.ambiguous => Ambiguous(message, sources),
      ErrorCode.unsupported => Unsupported(message, sources),
      ErrorCode.connectionRefused => ConnectionRefused(message, sources),
      ErrorCode.connectionAborted => ConnectionAborted(message, sources),
      ErrorCode.transportError => TransportError(message, sources),
      ErrorCode.listenerBindError => ListenerBindError(message, sources),
      ErrorCode.listenerAcceptError => ListenerAcceptError(message, sources),
      ErrorCode.storeError => StoreError(message, sources),
      ErrorCode.isDirectory => IsDirectory(message, sources),
      ErrorCode.notDirectory => NotDirectory(message, sources),
      ErrorCode.directoryNotEmpty => DirectoryNotEmpty(message, sources),
      ErrorCode.resourceBusy => ResourceBusy(message, sources),
      ErrorCode.runtimeInitializeError => RuntimeInitializeError(message, sources),
      ErrorCode.loggerInitializeError => LoggerInitializeError(message, sources),
      ErrorCode.configError => ConfigError(message, sources),
      ErrorCode.tlsCertificatesNotFound => TlsCertificatesNotFound(message, sources),
      ErrorCode.tlsCertificatesInvalid => TlsCertificatesInvalid(message, sources),
      ErrorCode.tlsKeysNotFound => TlsKeysNotFound(message, sources),
      ErrorCode.tlsConfigError => TlsConfigError(message, sources),
      ErrorCode.vfsDriverInstallError => VfsDriverInstallError(message, sources),
      ErrorCode.vfsOtherError => VfsOtherError(message, sources),
      ErrorCode.serviceAlreadyRunning => ServiceAlreadyRunning(message, sources),
      ErrorCode.storeDirUnspecified => StoreDirUnspecified(message, sources),
      ErrorCode.other => OuisyncException._(code, message, sources),
    };

  @override
  String toString() => [message].followedBy(sources).join(' → ');
}

/// Insuficient permission to perform the intended operation
class PermissionDenied extends OuisyncException {
  PermissionDenied([String? message, List<String> sources = const[]])
      : super._(ErrorCode.permissionDenied, message, sources);
}

/// Invalid input parameter
class InvalidInput extends OuisyncException {
  InvalidInput([String? message, List<String> sources = const[]])
      : super._(ErrorCode.invalidInput, message, sources);
}

/// Invalid data (e.g., malformed incoming message, config file, etc...)
class InvalidData extends OuisyncException {
  InvalidData([String? message, List<String> sources = const[]])
      : super._(ErrorCode.invalidData, message, sources);
}

/// Entry already exists
class AlreadyExists extends OuisyncException {
  AlreadyExists([String? message, List<String> sources = const[]])
      : super._(ErrorCode.alreadyExists, message, sources);
}

/// Entry not found
class NotFound extends OuisyncException {
  NotFound([String? message, List<String> sources = const[]])
      : super._(ErrorCode.notFound, message, sources);
}

/// Multiple matching entries found
class Ambiguous extends OuisyncException {
  Ambiguous([String? message, List<String> sources = const[]])
      : super._(ErrorCode.ambiguous, message, sources);
}

/// The indended operation is not supported
class Unsupported extends OuisyncException {
  Unsupported([String? message, List<String> sources = const[]])
      : super._(ErrorCode.unsupported, message, sources);
}

/// Failed to establish connection to the server
class ConnectionRefused extends OuisyncException {
  ConnectionRefused([String? message, List<String> sources = const[]])
      : super._(ErrorCode.connectionRefused, message, sources);
}

/// Connection aborted by the server
class ConnectionAborted extends OuisyncException {
  ConnectionAborted([String? message, List<String> sources = const[]])
      : super._(ErrorCode.connectionAborted, message, sources);
}

/// Failed to send or receive message
class TransportError extends OuisyncException {
  TransportError([String? message, List<String> sources = const[]])
      : super._(ErrorCode.transportError, message, sources);
}

/// Listener failed to bind to the specified address
class ListenerBindError extends OuisyncException {
  ListenerBindError([String? message, List<String> sources = const[]])
      : super._(ErrorCode.listenerBindError, message, sources);
}

/// Listener failed to accept client connection
class ListenerAcceptError extends OuisyncException {
  ListenerAcceptError([String? message, List<String> sources = const[]])
      : super._(ErrorCode.listenerAcceptError, message, sources);
}

/// Operation on the internal repository store failed
class StoreError extends OuisyncException {
  StoreError([String? message, List<String> sources = const[]])
      : super._(ErrorCode.storeError, message, sources);
}

/// Entry was expected to not be a directory but it is
class IsDirectory extends OuisyncException {
  IsDirectory([String? message, List<String> sources = const[]])
      : super._(ErrorCode.isDirectory, message, sources);
}

/// Entry was expected to be a directory but it isn't
class NotDirectory extends OuisyncException {
  NotDirectory([String? message, List<String> sources = const[]])
      : super._(ErrorCode.notDirectory, message, sources);
}

/// Directory was expected to be empty but it isn't
class DirectoryNotEmpty extends OuisyncException {
  DirectoryNotEmpty([String? message, List<String> sources = const[]])
      : super._(ErrorCode.directoryNotEmpty, message, sources);
}

/// File or directory is busy
class ResourceBusy extends OuisyncException {
  ResourceBusy([String? message, List<String> sources = const[]])
      : super._(ErrorCode.resourceBusy, message, sources);
}

/// Failed to initialize runtime
class RuntimeInitializeError extends OuisyncException {
  RuntimeInitializeError([String? message, List<String> sources = const[]])
      : super._(ErrorCode.runtimeInitializeError, message, sources);
}

/// Failed to initialize logger
class LoggerInitializeError extends OuisyncException {
  LoggerInitializeError([String? message, List<String> sources = const[]])
      : super._(ErrorCode.loggerInitializeError, message, sources);
}

/// Failed to read from or write into the config file
class ConfigError extends OuisyncException {
  ConfigError([String? message, List<String> sources = const[]])
      : super._(ErrorCode.configError, message, sources);
}

/// TLS certificated not found
class TlsCertificatesNotFound extends OuisyncException {
  TlsCertificatesNotFound([String? message, List<String> sources = const[]])
      : super._(ErrorCode.tlsCertificatesNotFound, message, sources);
}

/// TLS certificates failed to load
class TlsCertificatesInvalid extends OuisyncException {
  TlsCertificatesInvalid([String? message, List<String> sources = const[]])
      : super._(ErrorCode.tlsCertificatesInvalid, message, sources);
}

/// TLS keys not found
class TlsKeysNotFound extends OuisyncException {
  TlsKeysNotFound([String? message, List<String> sources = const[]])
      : super._(ErrorCode.tlsKeysNotFound, message, sources);
}

/// Failed to create TLS config
class TlsConfigError extends OuisyncException {
  TlsConfigError([String? message, List<String> sources = const[]])
      : super._(ErrorCode.tlsConfigError, message, sources);
}

/// Failed to install virtual filesystem driver
class VfsDriverInstallError extends OuisyncException {
  VfsDriverInstallError([String? message, List<String> sources = const[]])
      : super._(ErrorCode.vfsDriverInstallError, message, sources);
}

/// Unspecified virtual filesystem error
class VfsOtherError extends OuisyncException {
  VfsOtherError([String? message, List<String> sources = const[]])
      : super._(ErrorCode.vfsOtherError, message, sources);
}

/// Another instance of the service is already running
class ServiceAlreadyRunning extends OuisyncException {
  ServiceAlreadyRunning([String? message, List<String> sources = const[]])
      : super._(ErrorCode.serviceAlreadyRunning, message, sources);
}

/// Store directory is not specified
class StoreDirUnspecified extends OuisyncException {
  StoreDirUnspecified([String? message, List<String> sources = const[]])
      : super._(ErrorCode.storeDirUnspecified, message, sources);
}

enum LogLevel {
  error,
  warn,
  info,
  debug,
  trace,
  ;

  static LogLevel? fromInt(int n) {
    switch (n) {
      case 1: return LogLevel.error;
      case 2: return LogLevel.warn;
      case 3: return LogLevel.info;
      case 4: return LogLevel.debug;
      case 5: return LogLevel.trace;
      default: return null;
    }
  }

  int toInt() => switch (this) {
      LogLevel.error => 1,
      LogLevel.warn => 2,
      LogLevel.info => 3,
      LogLevel.debug => 4,
      LogLevel.trace => 5,
    };

  static LogLevel? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    p.packInt(toInt());
  }

}

final class MessageId {
  final int value;

  MessageId(this.value);

  void encode(Packer p) {
    p.packInt(value);
  }

  static MessageId? decode(Unpacker u) {
    final value = u.unpackInt();
    return value != null ? MessageId(value) : null;
  }

  @override
  operator==(Object other) =>
      other is MessageId &&
      other.value == value;

  @override
  int get hashCode => value.hashCode;
}

/// Edit of a single metadata entry.
final class MetadataEdit {
  /// The key of the entry.
  final String key;
  /// The current value of the entry or `None` if the entry does not exist yet. This is used for
  /// concurrency control - if the current value is different from this it's assumed it has been
  /// modified by some other task and the whole `RepositorySetMetadata` operation is rolled back.
  /// If that happens, the user should read the current value again, adjust the new value if
  /// needed and retry the operation.
  final String? oldValue;
  /// The value to set the entry to or `None` to remove the entry.
  final String? newValue;

  MetadataEdit({
    required this.key,
    required this.oldValue,
    required this.newValue,
  });

  void encode(Packer p) {
    p.packListLength(3);
    p.packString(key);
    _encodeNullable(p, oldValue, (p, e) => p.packString(e));
    _encodeNullable(p, newValue, (p, e) => p.packString(e));
  }

  static MetadataEdit? decode(Unpacker u) {
    switch (u.unpackListLength()) {
      case 0: return null;
      case 3: break;
      default: throw DecodeError();
    }
    return MetadataEdit(
      key: (u.unpackString())!,
      oldValue: u.unpackString(),
      newValue: u.unpackString(),
    );
  }

  @override
  operator==(Object other) =>
      other is MetadataEdit &&
      other.key == key &&
      other.oldValue == oldValue &&
      other.newValue == newValue;

  @override
  int get hashCode => Object.hash(
        key,
        oldValue,
        newValue,
      );
}

final class NetworkDefaults {
  final List<String> bind;
  final bool portForwardingEnabled;
  final bool localDiscoveryEnabled;

  NetworkDefaults({
    required this.bind,
    required this.portForwardingEnabled,
    required this.localDiscoveryEnabled,
  });

  void encode(Packer p) {
    p.packListLength(3);
    _encodeList(p, bind, (p, e) => p.packString(e));
    p.packBool(portForwardingEnabled);
    p.packBool(localDiscoveryEnabled);
  }

  static NetworkDefaults? decode(Unpacker u) {
    switch (u.unpackListLength()) {
      case 0: return null;
      case 3: break;
      default: throw DecodeError();
    }
    return NetworkDefaults(
      bind: _decodeList(u, (u) => (u.unpackString())!),
      portForwardingEnabled: (u.unpackBool())!,
      localDiscoveryEnabled: (u.unpackBool())!,
    );
  }

  @override
  operator==(Object other) =>
      other is NetworkDefaults &&
      _ListWrapper(other.bind) == _ListWrapper(bind) &&
      other.portForwardingEnabled == portForwardingEnabled &&
      other.localDiscoveryEnabled == localDiscoveryEnabled;

  @override
  int get hashCode => Object.hash(
        _ListWrapper(bind),
        portForwardingEnabled,
        localDiscoveryEnabled,
      );
}

final class DirectoryEntry {
  final String name;
  final EntryType entryType;

  DirectoryEntry({
    required this.name,
    required this.entryType,
  });

  void encode(Packer p) {
    p.packListLength(2);
    p.packString(name);
    entryType.encode(p);
  }

  static DirectoryEntry? decode(Unpacker u) {
    switch (u.unpackListLength()) {
      case 0: return null;
      case 2: break;
      default: throw DecodeError();
    }
    return DirectoryEntry(
      name: (u.unpackString())!,
      entryType: (EntryType.decode(u))!,
    );
  }

  @override
  operator==(Object other) =>
      other is DirectoryEntry &&
      other.name == name &&
      other.entryType == entryType;

  @override
  int get hashCode => Object.hash(
        name,
        entryType,
      );
}

final class QuotaInfo {
  final StorageSize? quota;
  final StorageSize size;

  QuotaInfo({
    required this.quota,
    required this.size,
  });

  void encode(Packer p) {
    p.packListLength(2);
    _encodeNullable(p, quota, (p, e) => e.encode(p));
    size.encode(p);
  }

  static QuotaInfo? decode(Unpacker u) {
    switch (u.unpackListLength()) {
      case 0: return null;
      case 2: break;
      default: throw DecodeError();
    }
    return QuotaInfo(
      quota: StorageSize.decode(u),
      size: (StorageSize.decode(u))!,
    );
  }

  @override
  operator==(Object other) =>
      other is QuotaInfo &&
      other.quota == quota &&
      other.size == size;

  @override
  int get hashCode => Object.hash(
        quota,
        size,
      );
}

final class FileHandle {
  final int value;

  FileHandle(this.value);

  void encode(Packer p) {
    p.packInt(value);
  }

  static FileHandle? decode(Unpacker u) {
    final value = u.unpackInt();
    return value != null ? FileHandle(value) : null;
  }

  @override
  operator==(Object other) =>
      other is FileHandle &&
      other.value == value;

  @override
  int get hashCode => value.hashCode;
}

final class RepositoryHandle {
  final int value;

  RepositoryHandle(this.value);

  void encode(Packer p) {
    p.packInt(value);
  }

  static RepositoryHandle? decode(Unpacker u) {
    final value = u.unpackInt();
    return value != null ? RepositoryHandle(value) : null;
  }

  @override
  operator==(Object other) =>
      other is RepositoryHandle &&
      other.value == value;

  @override
  int get hashCode => value.hashCode;
}

sealed class Request {
  void encode(Packer p) {
    switch (this) {
      case RequestFileClose(
        file: final file,
      ):
        p.packMapLength(1);
        p.packString('FileClose');
        p.packListLength(1);
        file.encode(p);
      case RequestFileFlush(
        file: final file,
      ):
        p.packMapLength(1);
        p.packString('FileFlush');
        p.packListLength(1);
        file.encode(p);
      case RequestFileGetLength(
        file: final file,
      ):
        p.packMapLength(1);
        p.packString('FileGetLength');
        p.packListLength(1);
        file.encode(p);
      case RequestFileGetProgress(
        file: final file,
      ):
        p.packMapLength(1);
        p.packString('FileGetProgress');
        p.packListLength(1);
        file.encode(p);
      case RequestFileRead(
        file: final file,
        offset: final offset,
        size: final size,
      ):
        p.packMapLength(1);
        p.packString('FileRead');
        p.packListLength(3);
        file.encode(p);
        p.packInt(offset);
        p.packInt(size);
      case RequestFileTruncate(
        file: final file,
        len: final len,
      ):
        p.packMapLength(1);
        p.packString('FileTruncate');
        p.packListLength(2);
        file.encode(p);
        p.packInt(len);
      case RequestFileWrite(
        file: final file,
        offset: final offset,
        data: final data,
      ):
        p.packMapLength(1);
        p.packString('FileWrite');
        p.packListLength(3);
        file.encode(p);
        p.packInt(offset);
        p.packBinary(data);
      case RequestMetricsBind(
        addr: final addr,
      ):
        p.packMapLength(1);
        p.packString('MetricsBind');
        p.packListLength(1);
        _encodeNullable(p, addr, (p, e) => p.packString(e));
      case RequestMetricsGetListenerAddr(
      ):
        p.packString('MetricsGetListenerAddr');
      case RequestRemoteControlBind(
        addr: final addr,
      ):
        p.packMapLength(1);
        p.packString('RemoteControlBind');
        p.packListLength(1);
        _encodeNullable(p, addr, (p, e) => p.packString(e));
      case RequestRemoteControlGetListenerAddr(
      ):
        p.packString('RemoteControlGetListenerAddr');
      case RequestRepositoryClose(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryClose');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryCreateDirectory(
        repo: final repo,
        path: final path,
      ):
        p.packMapLength(1);
        p.packString('RepositoryCreateDirectory');
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryCreateFile(
        repo: final repo,
        path: final path,
      ):
        p.packMapLength(1);
        p.packString('RepositoryCreateFile');
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryCreateMirror(
        repo: final repo,
        host: final host,
      ):
        p.packMapLength(1);
        p.packString('RepositoryCreateMirror');
        p.packListLength(2);
        repo.encode(p);
        p.packString(host);
      case RequestRepositoryDelete(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryDelete');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryDeleteMirror(
        repo: final repo,
        host: final host,
      ):
        p.packMapLength(1);
        p.packString('RepositoryDeleteMirror');
        p.packListLength(2);
        repo.encode(p);
        p.packString(host);
      case RequestRepositoryExport(
        repo: final repo,
        outputPath: final outputPath,
      ):
        p.packMapLength(1);
        p.packString('RepositoryExport');
        p.packListLength(2);
        repo.encode(p);
        p.packString(outputPath);
      case RequestRepositoryFileExists(
        repo: final repo,
        path: final path,
      ):
        p.packMapLength(1);
        p.packString('RepositoryFileExists');
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryGetAccessMode(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryGetAccessMode');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetBlockExpiration(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryGetBlockExpiration');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetCredentials(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryGetCredentials');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetEntryType(
        repo: final repo,
        path: final path,
      ):
        p.packMapLength(1);
        p.packString('RepositoryGetEntryType');
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryGetExpiration(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryGetExpiration');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetInfoHash(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryGetInfoHash');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetMetadata(
        repo: final repo,
        key: final key,
      ):
        p.packMapLength(1);
        p.packString('RepositoryGetMetadata');
        p.packListLength(2);
        repo.encode(p);
        p.packString(key);
      case RequestRepositoryGetMountPoint(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryGetMountPoint');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetPath(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryGetPath');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetQuota(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryGetQuota');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetStats(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryGetStats');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetSyncProgress(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryGetSyncProgress');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryIsDhtEnabled(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryIsDhtEnabled');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryIsPexEnabled(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryIsPexEnabled');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryIsSyncEnabled(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryIsSyncEnabled');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryMirrorExists(
        repo: final repo,
        host: final host,
      ):
        p.packMapLength(1);
        p.packString('RepositoryMirrorExists');
        p.packListLength(2);
        repo.encode(p);
        p.packString(host);
      case RequestRepositoryMount(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryMount');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryMove(
        repo: final repo,
        dst: final dst,
      ):
        p.packMapLength(1);
        p.packString('RepositoryMove');
        p.packListLength(2);
        repo.encode(p);
        p.packString(dst);
      case RequestRepositoryMoveEntry(
        repo: final repo,
        src: final src,
        dst: final dst,
      ):
        p.packMapLength(1);
        p.packString('RepositoryMoveEntry');
        p.packListLength(3);
        repo.encode(p);
        p.packString(src);
        p.packString(dst);
      case RequestRepositoryOpenFile(
        repo: final repo,
        path: final path,
      ):
        p.packMapLength(1);
        p.packString('RepositoryOpenFile');
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryReadDirectory(
        repo: final repo,
        path: final path,
      ):
        p.packMapLength(1);
        p.packString('RepositoryReadDirectory');
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryRemoveDirectory(
        repo: final repo,
        path: final path,
        recursive: final recursive,
      ):
        p.packMapLength(1);
        p.packString('RepositoryRemoveDirectory');
        p.packListLength(3);
        repo.encode(p);
        p.packString(path);
        p.packBool(recursive);
      case RequestRepositoryRemoveFile(
        repo: final repo,
        path: final path,
      ):
        p.packMapLength(1);
        p.packString('RepositoryRemoveFile');
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryResetAccess(
        repo: final repo,
        token: final token,
      ):
        p.packMapLength(1);
        p.packString('RepositoryResetAccess');
        p.packListLength(2);
        repo.encode(p);
        token.encode(p);
      case RequestRepositorySetAccess(
        repo: final repo,
        read: final read,
        write: final write,
      ):
        p.packMapLength(1);
        p.packString('RepositorySetAccess');
        p.packListLength(3);
        repo.encode(p);
        _encodeNullable(p, read, (p, e) => e.encode(p));
        _encodeNullable(p, write, (p, e) => e.encode(p));
      case RequestRepositorySetAccessMode(
        repo: final repo,
        accessMode: final accessMode,
        localSecret: final localSecret,
      ):
        p.packMapLength(1);
        p.packString('RepositorySetAccessMode');
        p.packListLength(3);
        repo.encode(p);
        accessMode.encode(p);
        _encodeNullable(p, localSecret, (p, e) => e.encode(p));
      case RequestRepositorySetBlockExpiration(
        repo: final repo,
        value: final value,
      ):
        p.packMapLength(1);
        p.packString('RepositorySetBlockExpiration');
        p.packListLength(2);
        repo.encode(p);
        _encodeNullable(p, value, (p, e) => _encodeDuration(p, e));
      case RequestRepositorySetCredentials(
        repo: final repo,
        credentials: final credentials,
      ):
        p.packMapLength(1);
        p.packString('RepositorySetCredentials');
        p.packListLength(2);
        repo.encode(p);
        p.packBinary(credentials);
      case RequestRepositorySetDhtEnabled(
        repo: final repo,
        enabled: final enabled,
      ):
        p.packMapLength(1);
        p.packString('RepositorySetDhtEnabled');
        p.packListLength(2);
        repo.encode(p);
        p.packBool(enabled);
      case RequestRepositorySetExpiration(
        repo: final repo,
        value: final value,
      ):
        p.packMapLength(1);
        p.packString('RepositorySetExpiration');
        p.packListLength(2);
        repo.encode(p);
        _encodeNullable(p, value, (p, e) => _encodeDuration(p, e));
      case RequestRepositorySetMetadata(
        repo: final repo,
        edits: final edits,
      ):
        p.packMapLength(1);
        p.packString('RepositorySetMetadata');
        p.packListLength(2);
        repo.encode(p);
        _encodeList(p, edits, (p, e) => e.encode(p));
      case RequestRepositorySetPexEnabled(
        repo: final repo,
        enabled: final enabled,
      ):
        p.packMapLength(1);
        p.packString('RepositorySetPexEnabled');
        p.packListLength(2);
        repo.encode(p);
        p.packBool(enabled);
      case RequestRepositorySetQuota(
        repo: final repo,
        value: final value,
      ):
        p.packMapLength(1);
        p.packString('RepositorySetQuota');
        p.packListLength(2);
        repo.encode(p);
        _encodeNullable(p, value, (p, e) => e.encode(p));
      case RequestRepositorySetSyncEnabled(
        repo: final repo,
        enabled: final enabled,
      ):
        p.packMapLength(1);
        p.packString('RepositorySetSyncEnabled');
        p.packListLength(2);
        repo.encode(p);
        p.packBool(enabled);
      case RequestRepositoryShare(
        repo: final repo,
        localSecret: final localSecret,
        accessMode: final accessMode,
      ):
        p.packMapLength(1);
        p.packString('RepositoryShare');
        p.packListLength(3);
        repo.encode(p);
        _encodeNullable(p, localSecret, (p, e) => e.encode(p));
        accessMode.encode(p);
      case RequestRepositorySubscribe(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositorySubscribe');
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryUnmount(
        repo: final repo,
      ):
        p.packMapLength(1);
        p.packString('RepositoryUnmount');
        p.packListLength(1);
        repo.encode(p);
      case RequestSessionAddUserProvidedPeers(
        addrs: final addrs,
      ):
        p.packMapLength(1);
        p.packString('SessionAddUserProvidedPeers');
        p.packListLength(1);
        _encodeList(p, addrs, (p, e) => p.packString(e));
      case RequestSessionBindNetwork(
        addrs: final addrs,
      ):
        p.packMapLength(1);
        p.packString('SessionBindNetwork');
        p.packListLength(1);
        _encodeList(p, addrs, (p, e) => p.packString(e));
      case RequestSessionCreateRepository(
        path: final path,
        readSecret: final readSecret,
        writeSecret: final writeSecret,
        token: final token,
        syncEnabled: final syncEnabled,
        dhtEnabled: final dhtEnabled,
        pexEnabled: final pexEnabled,
      ):
        p.packMapLength(1);
        p.packString('SessionCreateRepository');
        p.packListLength(7);
        p.packString(path);
        _encodeNullable(p, readSecret, (p, e) => e.encode(p));
        _encodeNullable(p, writeSecret, (p, e) => e.encode(p));
        _encodeNullable(p, token, (p, e) => e.encode(p));
        p.packBool(syncEnabled);
        p.packBool(dhtEnabled);
        p.packBool(pexEnabled);
      case RequestSessionDeleteRepositoryByName(
        name: final name,
      ):
        p.packMapLength(1);
        p.packString('SessionDeleteRepositoryByName');
        p.packListLength(1);
        p.packString(name);
      case RequestSessionDeriveSecretKey(
        password: final password,
        salt: final salt,
      ):
        p.packMapLength(1);
        p.packString('SessionDeriveSecretKey');
        p.packListLength(2);
        password.encode(p);
        salt.encode(p);
      case RequestSessionFindRepository(
        name: final name,
      ):
        p.packMapLength(1);
        p.packString('SessionFindRepository');
        p.packListLength(1);
        p.packString(name);
      case RequestSessionGeneratePasswordSalt(
      ):
        p.packString('SessionGeneratePasswordSalt');
      case RequestSessionGenerateSecretKey(
      ):
        p.packString('SessionGenerateSecretKey');
      case RequestSessionGetCurrentProtocolVersion(
      ):
        p.packString('SessionGetCurrentProtocolVersion');
      case RequestSessionGetDefaultBlockExpiration(
      ):
        p.packString('SessionGetDefaultBlockExpiration');
      case RequestSessionGetDefaultQuota(
      ):
        p.packString('SessionGetDefaultQuota');
      case RequestSessionGetDefaultRepositoryExpiration(
      ):
        p.packString('SessionGetDefaultRepositoryExpiration');
      case RequestSessionGetExternalAddrV4(
      ):
        p.packString('SessionGetExternalAddrV4');
      case RequestSessionGetExternalAddrV6(
      ):
        p.packString('SessionGetExternalAddrV6');
      case RequestSessionGetHighestSeenProtocolVersion(
      ):
        p.packString('SessionGetHighestSeenProtocolVersion');
      case RequestSessionGetLocalListenerAddrs(
      ):
        p.packString('SessionGetLocalListenerAddrs');
      case RequestSessionGetMountRoot(
      ):
        p.packString('SessionGetMountRoot');
      case RequestSessionGetNatBehavior(
      ):
        p.packString('SessionGetNatBehavior');
      case RequestSessionGetNetworkStats(
      ):
        p.packString('SessionGetNetworkStats');
      case RequestSessionGetPeers(
      ):
        p.packString('SessionGetPeers');
      case RequestSessionGetRemoteListenerAddrs(
        host: final host,
      ):
        p.packMapLength(1);
        p.packString('SessionGetRemoteListenerAddrs');
        p.packListLength(1);
        p.packString(host);
      case RequestSessionGetRuntimeId(
      ):
        p.packString('SessionGetRuntimeId');
      case RequestSessionGetShareTokenAccessMode(
        token: final token,
      ):
        p.packMapLength(1);
        p.packString('SessionGetShareTokenAccessMode');
        p.packListLength(1);
        token.encode(p);
      case RequestSessionGetShareTokenInfoHash(
        token: final token,
      ):
        p.packMapLength(1);
        p.packString('SessionGetShareTokenInfoHash');
        p.packListLength(1);
        token.encode(p);
      case RequestSessionGetShareTokenSuggestedName(
        token: final token,
      ):
        p.packMapLength(1);
        p.packString('SessionGetShareTokenSuggestedName');
        p.packListLength(1);
        token.encode(p);
      case RequestSessionGetStateMonitor(
        path: final path,
      ):
        p.packMapLength(1);
        p.packString('SessionGetStateMonitor');
        p.packListLength(1);
        _encodeList(p, path, (p, e) => e.encode(p));
      case RequestSessionGetStoreDir(
      ):
        p.packString('SessionGetStoreDir');
      case RequestSessionGetUserProvidedPeers(
      ):
        p.packString('SessionGetUserProvidedPeers');
      case RequestSessionInitNetwork(
        defaults: final defaults,
      ):
        p.packMapLength(1);
        p.packString('SessionInitNetwork');
        p.packListLength(1);
        defaults.encode(p);
      case RequestSessionIsLocalDiscoveryEnabled(
      ):
        p.packString('SessionIsLocalDiscoveryEnabled');
      case RequestSessionIsPexRecvEnabled(
      ):
        p.packString('SessionIsPexRecvEnabled');
      case RequestSessionIsPexSendEnabled(
      ):
        p.packString('SessionIsPexSendEnabled');
      case RequestSessionIsPortForwardingEnabled(
      ):
        p.packString('SessionIsPortForwardingEnabled');
      case RequestSessionListRepositories(
      ):
        p.packString('SessionListRepositories');
      case RequestSessionMirrorExists(
        token: final token,
        host: final host,
      ):
        p.packMapLength(1);
        p.packString('SessionMirrorExists');
        p.packListLength(2);
        token.encode(p);
        p.packString(host);
      case RequestSessionOpenRepository(
        path: final path,
        localSecret: final localSecret,
      ):
        p.packMapLength(1);
        p.packString('SessionOpenRepository');
        p.packListLength(2);
        p.packString(path);
        _encodeNullable(p, localSecret, (p, e) => e.encode(p));
      case RequestSessionRemoveUserProvidedPeers(
        addrs: final addrs,
      ):
        p.packMapLength(1);
        p.packString('SessionRemoveUserProvidedPeers');
        p.packListLength(1);
        _encodeList(p, addrs, (p, e) => p.packString(e));
      case RequestSessionSetDefaultBlockExpiration(
        value: final value,
      ):
        p.packMapLength(1);
        p.packString('SessionSetDefaultBlockExpiration');
        p.packListLength(1);
        _encodeNullable(p, value, (p, e) => _encodeDuration(p, e));
      case RequestSessionSetDefaultQuota(
        value: final value,
      ):
        p.packMapLength(1);
        p.packString('SessionSetDefaultQuota');
        p.packListLength(1);
        _encodeNullable(p, value, (p, e) => e.encode(p));
      case RequestSessionSetDefaultRepositoryExpiration(
        value: final value,
      ):
        p.packMapLength(1);
        p.packString('SessionSetDefaultRepositoryExpiration');
        p.packListLength(1);
        _encodeNullable(p, value, (p, e) => _encodeDuration(p, e));
      case RequestSessionSetLocalDiscoveryEnabled(
        enabled: final enabled,
      ):
        p.packMapLength(1);
        p.packString('SessionSetLocalDiscoveryEnabled');
        p.packListLength(1);
        p.packBool(enabled);
      case RequestSessionSetMountRoot(
        path: final path,
      ):
        p.packMapLength(1);
        p.packString('SessionSetMountRoot');
        p.packListLength(1);
        _encodeNullable(p, path, (p, e) => p.packString(e));
      case RequestSessionSetPexRecvEnabled(
        enabled: final enabled,
      ):
        p.packMapLength(1);
        p.packString('SessionSetPexRecvEnabled');
        p.packListLength(1);
        p.packBool(enabled);
      case RequestSessionSetPexSendEnabled(
        enabled: final enabled,
      ):
        p.packMapLength(1);
        p.packString('SessionSetPexSendEnabled');
        p.packListLength(1);
        p.packBool(enabled);
      case RequestSessionSetPortForwardingEnabled(
        enabled: final enabled,
      ):
        p.packMapLength(1);
        p.packString('SessionSetPortForwardingEnabled');
        p.packListLength(1);
        p.packBool(enabled);
      case RequestSessionSetStoreDir(
        path: final path,
      ):
        p.packMapLength(1);
        p.packString('SessionSetStoreDir');
        p.packListLength(1);
        p.packString(path);
      case RequestSessionSubscribeToNetwork(
      ):
        p.packString('SessionSubscribeToNetwork');
      case RequestSessionSubscribeToStateMonitor(
        path: final path,
      ):
        p.packMapLength(1);
        p.packString('SessionSubscribeToStateMonitor');
        p.packListLength(1);
        _encodeList(p, path, (p, e) => e.encode(p));
      case RequestSessionUnsubscribe(
        id: final id,
      ):
        p.packMapLength(1);
        p.packString('SessionUnsubscribe');
        p.packListLength(1);
        id.encode(p);
      case RequestSessionValidateShareToken(
        token: final token,
      ):
        p.packMapLength(1);
        p.packString('SessionValidateShareToken');
        p.packListLength(1);
        p.packString(token);
    }
  }

}

class RequestFileClose extends Request {
  final FileHandle file;

  RequestFileClose({
    required this.file,
  });
}

class RequestFileFlush extends Request {
  final FileHandle file;

  RequestFileFlush({
    required this.file,
  });
}

class RequestFileGetLength extends Request {
  final FileHandle file;

  RequestFileGetLength({
    required this.file,
  });
}

class RequestFileGetProgress extends Request {
  final FileHandle file;

  RequestFileGetProgress({
    required this.file,
  });
}

class RequestFileRead extends Request {
  final FileHandle file;
  final int offset;
  final int size;

  RequestFileRead({
    required this.file,
    required this.offset,
    required this.size,
  });
}

class RequestFileTruncate extends Request {
  final FileHandle file;
  final int len;

  RequestFileTruncate({
    required this.file,
    required this.len,
  });
}

class RequestFileWrite extends Request {
  final FileHandle file;
  final int offset;
  final List<int> data;

  RequestFileWrite({
    required this.file,
    required this.offset,
    required this.data,
  });
}

class RequestMetricsBind extends Request {
  final String? addr;

  RequestMetricsBind({
    required this.addr,
  });
}

class RequestMetricsGetListenerAddr extends Request {
  RequestMetricsGetListenerAddr();
}

class RequestRemoteControlBind extends Request {
  final String? addr;

  RequestRemoteControlBind({
    required this.addr,
  });
}

class RequestRemoteControlGetListenerAddr extends Request {
  RequestRemoteControlGetListenerAddr();
}

class RequestRepositoryClose extends Request {
  final RepositoryHandle repo;

  RequestRepositoryClose({
    required this.repo,
  });
}

class RequestRepositoryCreateDirectory extends Request {
  final RepositoryHandle repo;
  final String path;

  RequestRepositoryCreateDirectory({
    required this.repo,
    required this.path,
  });
}

class RequestRepositoryCreateFile extends Request {
  final RepositoryHandle repo;
  final String path;

  RequestRepositoryCreateFile({
    required this.repo,
    required this.path,
  });
}

class RequestRepositoryCreateMirror extends Request {
  final RepositoryHandle repo;
  final String host;

  RequestRepositoryCreateMirror({
    required this.repo,
    required this.host,
  });
}

class RequestRepositoryDelete extends Request {
  final RepositoryHandle repo;

  RequestRepositoryDelete({
    required this.repo,
  });
}

class RequestRepositoryDeleteMirror extends Request {
  final RepositoryHandle repo;
  final String host;

  RequestRepositoryDeleteMirror({
    required this.repo,
    required this.host,
  });
}

class RequestRepositoryExport extends Request {
  final RepositoryHandle repo;
  final String outputPath;

  RequestRepositoryExport({
    required this.repo,
    required this.outputPath,
  });
}

class RequestRepositoryFileExists extends Request {
  final RepositoryHandle repo;
  final String path;

  RequestRepositoryFileExists({
    required this.repo,
    required this.path,
  });
}

class RequestRepositoryGetAccessMode extends Request {
  final RepositoryHandle repo;

  RequestRepositoryGetAccessMode({
    required this.repo,
  });
}

class RequestRepositoryGetBlockExpiration extends Request {
  final RepositoryHandle repo;

  RequestRepositoryGetBlockExpiration({
    required this.repo,
  });
}

class RequestRepositoryGetCredentials extends Request {
  final RepositoryHandle repo;

  RequestRepositoryGetCredentials({
    required this.repo,
  });
}

class RequestRepositoryGetEntryType extends Request {
  final RepositoryHandle repo;
  final String path;

  RequestRepositoryGetEntryType({
    required this.repo,
    required this.path,
  });
}

class RequestRepositoryGetExpiration extends Request {
  final RepositoryHandle repo;

  RequestRepositoryGetExpiration({
    required this.repo,
  });
}

class RequestRepositoryGetInfoHash extends Request {
  final RepositoryHandle repo;

  RequestRepositoryGetInfoHash({
    required this.repo,
  });
}

class RequestRepositoryGetMetadata extends Request {
  final RepositoryHandle repo;
  final String key;

  RequestRepositoryGetMetadata({
    required this.repo,
    required this.key,
  });
}

class RequestRepositoryGetMountPoint extends Request {
  final RepositoryHandle repo;

  RequestRepositoryGetMountPoint({
    required this.repo,
  });
}

class RequestRepositoryGetPath extends Request {
  final RepositoryHandle repo;

  RequestRepositoryGetPath({
    required this.repo,
  });
}

class RequestRepositoryGetQuota extends Request {
  final RepositoryHandle repo;

  RequestRepositoryGetQuota({
    required this.repo,
  });
}

class RequestRepositoryGetStats extends Request {
  final RepositoryHandle repo;

  RequestRepositoryGetStats({
    required this.repo,
  });
}

class RequestRepositoryGetSyncProgress extends Request {
  final RepositoryHandle repo;

  RequestRepositoryGetSyncProgress({
    required this.repo,
  });
}

class RequestRepositoryIsDhtEnabled extends Request {
  final RepositoryHandle repo;

  RequestRepositoryIsDhtEnabled({
    required this.repo,
  });
}

class RequestRepositoryIsPexEnabled extends Request {
  final RepositoryHandle repo;

  RequestRepositoryIsPexEnabled({
    required this.repo,
  });
}

class RequestRepositoryIsSyncEnabled extends Request {
  final RepositoryHandle repo;

  RequestRepositoryIsSyncEnabled({
    required this.repo,
  });
}

class RequestRepositoryMirrorExists extends Request {
  final RepositoryHandle repo;
  final String host;

  RequestRepositoryMirrorExists({
    required this.repo,
    required this.host,
  });
}

class RequestRepositoryMount extends Request {
  final RepositoryHandle repo;

  RequestRepositoryMount({
    required this.repo,
  });
}

class RequestRepositoryMove extends Request {
  final RepositoryHandle repo;
  final String dst;

  RequestRepositoryMove({
    required this.repo,
    required this.dst,
  });
}

class RequestRepositoryMoveEntry extends Request {
  final RepositoryHandle repo;
  final String src;
  final String dst;

  RequestRepositoryMoveEntry({
    required this.repo,
    required this.src,
    required this.dst,
  });
}

class RequestRepositoryOpenFile extends Request {
  final RepositoryHandle repo;
  final String path;

  RequestRepositoryOpenFile({
    required this.repo,
    required this.path,
  });
}

class RequestRepositoryReadDirectory extends Request {
  final RepositoryHandle repo;
  final String path;

  RequestRepositoryReadDirectory({
    required this.repo,
    required this.path,
  });
}

class RequestRepositoryRemoveDirectory extends Request {
  final RepositoryHandle repo;
  final String path;
  final bool recursive;

  RequestRepositoryRemoveDirectory({
    required this.repo,
    required this.path,
    required this.recursive,
  });
}

class RequestRepositoryRemoveFile extends Request {
  final RepositoryHandle repo;
  final String path;

  RequestRepositoryRemoveFile({
    required this.repo,
    required this.path,
  });
}

class RequestRepositoryResetAccess extends Request {
  final RepositoryHandle repo;
  final ShareToken token;

  RequestRepositoryResetAccess({
    required this.repo,
    required this.token,
  });
}

class RequestRepositorySetAccess extends Request {
  final RepositoryHandle repo;
  final AccessChange? read;
  final AccessChange? write;

  RequestRepositorySetAccess({
    required this.repo,
    required this.read,
    required this.write,
  });
}

class RequestRepositorySetAccessMode extends Request {
  final RepositoryHandle repo;
  final AccessMode accessMode;
  final LocalSecret? localSecret;

  RequestRepositorySetAccessMode({
    required this.repo,
    required this.accessMode,
    required this.localSecret,
  });
}

class RequestRepositorySetBlockExpiration extends Request {
  final RepositoryHandle repo;
  final Duration? value;

  RequestRepositorySetBlockExpiration({
    required this.repo,
    required this.value,
  });
}

class RequestRepositorySetCredentials extends Request {
  final RepositoryHandle repo;
  final List<int> credentials;

  RequestRepositorySetCredentials({
    required this.repo,
    required this.credentials,
  });
}

class RequestRepositorySetDhtEnabled extends Request {
  final RepositoryHandle repo;
  final bool enabled;

  RequestRepositorySetDhtEnabled({
    required this.repo,
    required this.enabled,
  });
}

class RequestRepositorySetExpiration extends Request {
  final RepositoryHandle repo;
  final Duration? value;

  RequestRepositorySetExpiration({
    required this.repo,
    required this.value,
  });
}

class RequestRepositorySetMetadata extends Request {
  final RepositoryHandle repo;
  final List<MetadataEdit> edits;

  RequestRepositorySetMetadata({
    required this.repo,
    required this.edits,
  });
}

class RequestRepositorySetPexEnabled extends Request {
  final RepositoryHandle repo;
  final bool enabled;

  RequestRepositorySetPexEnabled({
    required this.repo,
    required this.enabled,
  });
}

class RequestRepositorySetQuota extends Request {
  final RepositoryHandle repo;
  final StorageSize? value;

  RequestRepositorySetQuota({
    required this.repo,
    required this.value,
  });
}

class RequestRepositorySetSyncEnabled extends Request {
  final RepositoryHandle repo;
  final bool enabled;

  RequestRepositorySetSyncEnabled({
    required this.repo,
    required this.enabled,
  });
}

class RequestRepositoryShare extends Request {
  final RepositoryHandle repo;
  final LocalSecret? localSecret;
  final AccessMode accessMode;

  RequestRepositoryShare({
    required this.repo,
    required this.localSecret,
    required this.accessMode,
  });
}

class RequestRepositorySubscribe extends Request {
  final RepositoryHandle repo;

  RequestRepositorySubscribe({
    required this.repo,
  });
}

class RequestRepositoryUnmount extends Request {
  final RepositoryHandle repo;

  RequestRepositoryUnmount({
    required this.repo,
  });
}

class RequestSessionAddUserProvidedPeers extends Request {
  final List<String> addrs;

  RequestSessionAddUserProvidedPeers({
    required this.addrs,
  });
}

class RequestSessionBindNetwork extends Request {
  final List<String> addrs;

  RequestSessionBindNetwork({
    required this.addrs,
  });
}

class RequestSessionCreateRepository extends Request {
  final String path;
  final SetLocalSecret? readSecret;
  final SetLocalSecret? writeSecret;
  final ShareToken? token;
  final bool syncEnabled;
  final bool dhtEnabled;
  final bool pexEnabled;

  RequestSessionCreateRepository({
    required this.path,
    required this.readSecret,
    required this.writeSecret,
    required this.token,
    required this.syncEnabled,
    required this.dhtEnabled,
    required this.pexEnabled,
  });
}

class RequestSessionDeleteRepositoryByName extends Request {
  final String name;

  RequestSessionDeleteRepositoryByName({
    required this.name,
  });
}

class RequestSessionDeriveSecretKey extends Request {
  final Password password;
  final PasswordSalt salt;

  RequestSessionDeriveSecretKey({
    required this.password,
    required this.salt,
  });
}

class RequestSessionFindRepository extends Request {
  final String name;

  RequestSessionFindRepository({
    required this.name,
  });
}

class RequestSessionGeneratePasswordSalt extends Request {
  RequestSessionGeneratePasswordSalt();
}

class RequestSessionGenerateSecretKey extends Request {
  RequestSessionGenerateSecretKey();
}

class RequestSessionGetCurrentProtocolVersion extends Request {
  RequestSessionGetCurrentProtocolVersion();
}

class RequestSessionGetDefaultBlockExpiration extends Request {
  RequestSessionGetDefaultBlockExpiration();
}

class RequestSessionGetDefaultQuota extends Request {
  RequestSessionGetDefaultQuota();
}

class RequestSessionGetDefaultRepositoryExpiration extends Request {
  RequestSessionGetDefaultRepositoryExpiration();
}

class RequestSessionGetExternalAddrV4 extends Request {
  RequestSessionGetExternalAddrV4();
}

class RequestSessionGetExternalAddrV6 extends Request {
  RequestSessionGetExternalAddrV6();
}

class RequestSessionGetHighestSeenProtocolVersion extends Request {
  RequestSessionGetHighestSeenProtocolVersion();
}

class RequestSessionGetLocalListenerAddrs extends Request {
  RequestSessionGetLocalListenerAddrs();
}

class RequestSessionGetMountRoot extends Request {
  RequestSessionGetMountRoot();
}

class RequestSessionGetNatBehavior extends Request {
  RequestSessionGetNatBehavior();
}

class RequestSessionGetNetworkStats extends Request {
  RequestSessionGetNetworkStats();
}

class RequestSessionGetPeers extends Request {
  RequestSessionGetPeers();
}

class RequestSessionGetRemoteListenerAddrs extends Request {
  final String host;

  RequestSessionGetRemoteListenerAddrs({
    required this.host,
  });
}

class RequestSessionGetRuntimeId extends Request {
  RequestSessionGetRuntimeId();
}

class RequestSessionGetShareTokenAccessMode extends Request {
  final ShareToken token;

  RequestSessionGetShareTokenAccessMode({
    required this.token,
  });
}

class RequestSessionGetShareTokenInfoHash extends Request {
  final ShareToken token;

  RequestSessionGetShareTokenInfoHash({
    required this.token,
  });
}

class RequestSessionGetShareTokenSuggestedName extends Request {
  final ShareToken token;

  RequestSessionGetShareTokenSuggestedName({
    required this.token,
  });
}

class RequestSessionGetStateMonitor extends Request {
  final List<MonitorId> path;

  RequestSessionGetStateMonitor({
    required this.path,
  });
}

class RequestSessionGetStoreDir extends Request {
  RequestSessionGetStoreDir();
}

class RequestSessionGetUserProvidedPeers extends Request {
  RequestSessionGetUserProvidedPeers();
}

class RequestSessionInitNetwork extends Request {
  final NetworkDefaults defaults;

  RequestSessionInitNetwork({
    required this.defaults,
  });
}

class RequestSessionIsLocalDiscoveryEnabled extends Request {
  RequestSessionIsLocalDiscoveryEnabled();
}

class RequestSessionIsPexRecvEnabled extends Request {
  RequestSessionIsPexRecvEnabled();
}

class RequestSessionIsPexSendEnabled extends Request {
  RequestSessionIsPexSendEnabled();
}

class RequestSessionIsPortForwardingEnabled extends Request {
  RequestSessionIsPortForwardingEnabled();
}

class RequestSessionListRepositories extends Request {
  RequestSessionListRepositories();
}

class RequestSessionMirrorExists extends Request {
  final ShareToken token;
  final String host;

  RequestSessionMirrorExists({
    required this.token,
    required this.host,
  });
}

class RequestSessionOpenRepository extends Request {
  final String path;
  final LocalSecret? localSecret;

  RequestSessionOpenRepository({
    required this.path,
    required this.localSecret,
  });
}

class RequestSessionRemoveUserProvidedPeers extends Request {
  final List<String> addrs;

  RequestSessionRemoveUserProvidedPeers({
    required this.addrs,
  });
}

class RequestSessionSetDefaultBlockExpiration extends Request {
  final Duration? value;

  RequestSessionSetDefaultBlockExpiration({
    required this.value,
  });
}

class RequestSessionSetDefaultQuota extends Request {
  final StorageSize? value;

  RequestSessionSetDefaultQuota({
    required this.value,
  });
}

class RequestSessionSetDefaultRepositoryExpiration extends Request {
  final Duration? value;

  RequestSessionSetDefaultRepositoryExpiration({
    required this.value,
  });
}

class RequestSessionSetLocalDiscoveryEnabled extends Request {
  final bool enabled;

  RequestSessionSetLocalDiscoveryEnabled({
    required this.enabled,
  });
}

class RequestSessionSetMountRoot extends Request {
  final String? path;

  RequestSessionSetMountRoot({
    required this.path,
  });
}

class RequestSessionSetPexRecvEnabled extends Request {
  final bool enabled;

  RequestSessionSetPexRecvEnabled({
    required this.enabled,
  });
}

class RequestSessionSetPexSendEnabled extends Request {
  final bool enabled;

  RequestSessionSetPexSendEnabled({
    required this.enabled,
  });
}

class RequestSessionSetPortForwardingEnabled extends Request {
  final bool enabled;

  RequestSessionSetPortForwardingEnabled({
    required this.enabled,
  });
}

class RequestSessionSetStoreDir extends Request {
  final String path;

  RequestSessionSetStoreDir({
    required this.path,
  });
}

class RequestSessionSubscribeToNetwork extends Request {
  RequestSessionSubscribeToNetwork();
}

class RequestSessionSubscribeToStateMonitor extends Request {
  final List<MonitorId> path;

  RequestSessionSubscribeToStateMonitor({
    required this.path,
  });
}

class RequestSessionUnsubscribe extends Request {
  final MessageId id;

  RequestSessionUnsubscribe({
    required this.id,
  });
}

class RequestSessionValidateShareToken extends Request {
  final String token;

  RequestSessionValidateShareToken({
    required this.token,
  });
}

sealed class Response {
  static Response? decode(Unpacker u) {
    try {
      switch (u.unpackString()) {
        case "None": return ResponseNone();
        case "RepositoryEvent": return ResponseRepositoryEvent();
        case "StateMonitorEvent": return ResponseStateMonitorEvent();
        case null: return null;
        default: throw DecodeError();
      }
    } on FormatException {
      if (u.unpackMapLength() != 1) throw DecodeError();
      switch (u.unpackString()) {
        case "AccessMode":
          return ResponseAccessMode(
            (AccessMode.decode(u))!,
          );
        case "Bool":
          return ResponseBool(
            (u.unpackBool())!,
          );
        case "Bytes":
          return ResponseBytes(
            u.unpackBinary(),
          );
        case "DirectoryEntries":
          return ResponseDirectoryEntries(
            _decodeList(u, (u) => (DirectoryEntry.decode(u))!),
          );
        case "Duration":
          return ResponseDuration(
            (_decodeDuration(u))!,
          );
        case "EntryType":
          return ResponseEntryType(
            (EntryType.decode(u))!,
          );
        case "File":
          return ResponseFile(
            (FileHandle.decode(u))!,
          );
        case "NatBehavior":
          return ResponseNatBehavior(
            (NatBehavior.decode(u))!,
          );
        case "NetworkEvent":
          return ResponseNetworkEvent(
            (NetworkEvent.decode(u))!,
          );
        case "PasswordSalt":
          return ResponsePasswordSalt(
            (PasswordSalt.decode(u))!,
          );
        case "Path":
          return ResponsePath(
            (u.unpackString())!,
          );
        case "PeerAddrs":
          return ResponsePeerAddrs(
            _decodeList(u, (u) => (u.unpackString())!),
          );
        case "PeerInfos":
          return ResponsePeerInfos(
            _decodeList(u, (u) => (PeerInfo.decode(u))!),
          );
        case "Progress":
          return ResponseProgress(
            (Progress.decode(u))!,
          );
        case "PublicRuntimeId":
          return ResponsePublicRuntimeId(
            (PublicRuntimeId.decode(u))!,
          );
        case "QuotaInfo":
          return ResponseQuotaInfo(
            (QuotaInfo.decode(u))!,
          );
        case "Repositories":
          return ResponseRepositories(
            _decodeMap(u, (u) => (u.unpackString())!, (u) => (RepositoryHandle.decode(u))!),
          );
        case "Repository":
          return ResponseRepository(
            (RepositoryHandle.decode(u))!,
          );
        case "SecretKey":
          return ResponseSecretKey(
            (SecretKey.decode(u))!,
          );
        case "ShareToken":
          return ResponseShareToken(
            (ShareToken.decode(u))!,
          );
        case "SocketAddr":
          return ResponseSocketAddr(
            (u.unpackString())!,
          );
        case "StateMonitor":
          return ResponseStateMonitor(
            (StateMonitorNode.decode(u))!,
          );
        case "Stats":
          return ResponseStats(
            (Stats.decode(u))!,
          );
        case "StorageSize":
          return ResponseStorageSize(
            (StorageSize.decode(u))!,
          );
        case "String":
          return ResponseString(
            (u.unpackString())!,
          );
        case "U16":
          return ResponseU16(
            (u.unpackInt())!,
          );
        case "U64":
          return ResponseU64(
            (u.unpackInt())!,
          );
        case null: return null;
        default: throw DecodeError();
      }
    }
  }

}

class ResponseAccessMode extends Response {
  final AccessMode value;

  ResponseAccessMode(this.value);
}

class ResponseBool extends Response {
  final bool value;

  ResponseBool(this.value);
}

class ResponseBytes extends Response {
  final List<int> value;

  ResponseBytes(this.value);
}

class ResponseDirectoryEntries extends Response {
  final List<DirectoryEntry> value;

  ResponseDirectoryEntries(this.value);
}

class ResponseDuration extends Response {
  final Duration value;

  ResponseDuration(this.value);
}

class ResponseEntryType extends Response {
  final EntryType value;

  ResponseEntryType(this.value);
}

class ResponseFile extends Response {
  final FileHandle value;

  ResponseFile(this.value);
}

class ResponseNatBehavior extends Response {
  final NatBehavior value;

  ResponseNatBehavior(this.value);
}

class ResponseNetworkEvent extends Response {
  final NetworkEvent value;

  ResponseNetworkEvent(this.value);
}

class ResponseNone extends Response {
  ResponseNone();
}

class ResponsePasswordSalt extends Response {
  final PasswordSalt value;

  ResponsePasswordSalt(this.value);
}

class ResponsePath extends Response {
  final String value;

  ResponsePath(this.value);
}

class ResponsePeerAddrs extends Response {
  final List<String> value;

  ResponsePeerAddrs(this.value);
}

class ResponsePeerInfos extends Response {
  final List<PeerInfo> value;

  ResponsePeerInfos(this.value);
}

class ResponseProgress extends Response {
  final Progress value;

  ResponseProgress(this.value);
}

class ResponsePublicRuntimeId extends Response {
  final PublicRuntimeId value;

  ResponsePublicRuntimeId(this.value);
}

class ResponseQuotaInfo extends Response {
  final QuotaInfo value;

  ResponseQuotaInfo(this.value);
}

class ResponseRepositories extends Response {
  final Map<String, RepositoryHandle> value;

  ResponseRepositories(this.value);
}

class ResponseRepository extends Response {
  final RepositoryHandle value;

  ResponseRepository(this.value);
}

class ResponseRepositoryEvent extends Response {
  ResponseRepositoryEvent();
}

class ResponseSecretKey extends Response {
  final SecretKey value;

  ResponseSecretKey(this.value);
}

class ResponseShareToken extends Response {
  final ShareToken value;

  ResponseShareToken(this.value);
}

class ResponseSocketAddr extends Response {
  final String value;

  ResponseSocketAddr(this.value);
}

class ResponseStateMonitor extends Response {
  final StateMonitorNode value;

  ResponseStateMonitor(this.value);
}

class ResponseStateMonitorEvent extends Response {
  ResponseStateMonitorEvent();
}

class ResponseStats extends Response {
  final Stats value;

  ResponseStats(this.value);
}

class ResponseStorageSize extends Response {
  final StorageSize value;

  ResponseStorageSize(this.value);
}

class ResponseString extends Response {
  final String value;

  ResponseString(this.value);
}

class ResponseU16 extends Response {
  final int value;

  ResponseU16(this.value);
}

class ResponseU64 extends Response {
  final int value;

  ResponseU64(this.value);
}

class Session {
  final Client client;

  Session(this.client);

  Future<void> addUserProvidedPeers(
    List<String> addrs,
  ) async {
    final request = RequestSessionAddUserProvidedPeers(
      addrs: addrs,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> bindNetwork(
    List<String> addrs,
  ) async {
    final request = RequestSessionBindNetwork(
      addrs: addrs,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<Repository> createRepository({
    required String path,
    SetLocalSecret? readSecret,
    SetLocalSecret? writeSecret,
    ShareToken? token,
    bool syncEnabled = false,
    bool dhtEnabled = false,
    bool pexEnabled = false,
  }) async {
    final request = RequestSessionCreateRepository(
      path: path,
      readSecret: readSecret,
      writeSecret: writeSecret,
      token: token,
      syncEnabled: syncEnabled,
      dhtEnabled: dhtEnabled,
      pexEnabled: pexEnabled,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseRepository(value: final value): return Repository(client, value);
      default: throw UnexpectedResponse();
    }
  }

  /// Delete a repository with the given name.
  Future<void> deleteRepositoryByName(
    String name,
  ) async {
    final request = RequestSessionDeleteRepositoryByName(
      name: name,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<SecretKey> deriveSecretKey(
    Password password,
    PasswordSalt salt,
  ) async {
    final request = RequestSessionDeriveSecretKey(
      password: password,
      salt: salt,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseSecretKey(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<Repository> findRepository(
    String name,
  ) async {
    final request = RequestSessionFindRepository(
      name: name,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseRepository(value: final value): return Repository(client, value);
      default: throw UnexpectedResponse();
    }
  }

  Future<PasswordSalt> generatePasswordSalt(
  ) async {
    final request = RequestSessionGeneratePasswordSalt(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponsePasswordSalt(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<SecretKey> generateSecretKey(
  ) async {
    final request = RequestSessionGenerateSecretKey(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseSecretKey(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<int> getCurrentProtocolVersion(
  ) async {
    final request = RequestSessionGetCurrentProtocolVersion(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseU64(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<Duration?> getDefaultBlockExpiration(
  ) async {
    final request = RequestSessionGetDefaultBlockExpiration(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseDuration(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  Future<StorageSize?> getDefaultQuota(
  ) async {
    final request = RequestSessionGetDefaultQuota(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseStorageSize(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  Future<Duration?> getDefaultRepositoryExpiration(
  ) async {
    final request = RequestSessionGetDefaultRepositoryExpiration(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseDuration(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  Future<String?> getExternalAddrV4(
  ) async {
    final request = RequestSessionGetExternalAddrV4(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseSocketAddr(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  Future<String?> getExternalAddrV6(
  ) async {
    final request = RequestSessionGetExternalAddrV6(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseSocketAddr(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  Future<int> getHighestSeenProtocolVersion(
  ) async {
    final request = RequestSessionGetHighestSeenProtocolVersion(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseU64(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<List<String>> getLocalListenerAddrs(
  ) async {
    final request = RequestSessionGetLocalListenerAddrs(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponsePeerAddrs(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<String?> getMountRoot(
  ) async {
    final request = RequestSessionGetMountRoot(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponsePath(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  Future<NatBehavior?> getNatBehavior(
  ) async {
    final request = RequestSessionGetNatBehavior(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNatBehavior(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  Future<Stats> getNetworkStats(
  ) async {
    final request = RequestSessionGetNetworkStats(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseStats(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<List<PeerInfo>> getPeers(
  ) async {
    final request = RequestSessionGetPeers(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponsePeerInfos(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<List<String>> getRemoteListenerAddrs(
    String host,
  ) async {
    final request = RequestSessionGetRemoteListenerAddrs(
      host: host,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponsePeerAddrs(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<PublicRuntimeId> getRuntimeId(
  ) async {
    final request = RequestSessionGetRuntimeId(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponsePublicRuntimeId(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<AccessMode> getShareTokenAccessMode(
    ShareToken token,
  ) async {
    final request = RequestSessionGetShareTokenAccessMode(
      token: token,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseAccessMode(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  /// Return the info-hash of the repository corresponding to the given token, formatted as hex
  /// string.
  ///
  /// See also: [repository_get_info_hash]
  Future<String> getShareTokenInfoHash(
    ShareToken token,
  ) async {
    final request = RequestSessionGetShareTokenInfoHash(
      token: token,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseString(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<String> getShareTokenSuggestedName(
    ShareToken token,
  ) async {
    final request = RequestSessionGetShareTokenSuggestedName(
      token: token,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseString(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<StateMonitorNode?> getStateMonitor(
    List<MonitorId> path,
  ) async {
    final request = RequestSessionGetStateMonitor(
      path: path,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseStateMonitor(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  Future<String?> getStoreDir(
  ) async {
    final request = RequestSessionGetStoreDir(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponsePath(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  Future<List<String>> getUserProvidedPeers(
  ) async {
    final request = RequestSessionGetUserProvidedPeers(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponsePeerAddrs(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  /// Initializes the network according to the stored configuration. If a particular network
  /// parameter is not yet configured, falls back to the given defaults.
  Future<void> initNetwork(
    NetworkDefaults defaults,
  ) async {
    final request = RequestSessionInitNetwork(
      defaults: defaults,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<bool> isLocalDiscoveryEnabled(
  ) async {
    final request = RequestSessionIsLocalDiscoveryEnabled(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  /// Checks whether accepting peers discovered on the peer exchange is enabled.
  Future<bool> isPexRecvEnabled(
  ) async {
    final request = RequestSessionIsPexRecvEnabled(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<bool> isPexSendEnabled(
  ) async {
    final request = RequestSessionIsPexSendEnabled(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<bool> isPortForwardingEnabled(
  ) async {
    final request = RequestSessionIsPortForwardingEnabled(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<Map<String, Repository>> listRepositories(
  ) async {
    final request = RequestSessionListRepositories(
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseRepositories(value: final value): return value.map((k, v) => MapEntry(k, Repository(client, v)));
      default: throw UnexpectedResponse();
    }
  }

  Future<bool> mirrorExists(
    ShareToken token,
    String host,
  ) async {
    final request = RequestSessionMirrorExists(
      token: token,
      host: host,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<Repository> openRepository({
    required String path,
    LocalSecret? localSecret,
  }) async {
    final request = RequestSessionOpenRepository(
      path: path,
      localSecret: localSecret,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseRepository(value: final value): return Repository(client, value);
      default: throw UnexpectedResponse();
    }
  }

  Future<void> removeUserProvidedPeers(
    List<String> addrs,
  ) async {
    final request = RequestSessionRemoveUserProvidedPeers(
      addrs: addrs,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setDefaultBlockExpiration(
    Duration? value,
  ) async {
    final request = RequestSessionSetDefaultBlockExpiration(
      value: value,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setDefaultQuota(
    StorageSize? value,
  ) async {
    final request = RequestSessionSetDefaultQuota(
      value: value,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setDefaultRepositoryExpiration(
    Duration? value,
  ) async {
    final request = RequestSessionSetDefaultRepositoryExpiration(
      value: value,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setLocalDiscoveryEnabled(
    bool enabled,
  ) async {
    final request = RequestSessionSetLocalDiscoveryEnabled(
      enabled: enabled,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setMountRoot(
    String? path,
  ) async {
    final request = RequestSessionSetMountRoot(
      path: path,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setPexRecvEnabled(
    bool enabled,
  ) async {
    final request = RequestSessionSetPexRecvEnabled(
      enabled: enabled,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setPexSendEnabled(
    bool enabled,
  ) async {
    final request = RequestSessionSetPexSendEnabled(
      enabled: enabled,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setPortForwardingEnabled(
    bool enabled,
  ) async {
    final request = RequestSessionSetPortForwardingEnabled(
      enabled: enabled,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setStoreDir(
    String path,
  ) async {
    final request = RequestSessionSetStoreDir(
      path: path,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<ShareToken> validateShareToken(
    String token,
  ) async {
    final request = RequestSessionValidateShareToken(
      token: token,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseShareToken(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

}

class Repository {
  final Client client;
  final RepositoryHandle handle;

  Repository(this.client, this.handle);

  Future<void> close(
  ) async {
    final request = RequestRepositoryClose(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> createDirectory(
    String path,
  ) async {
    final request = RequestRepositoryCreateDirectory(
      repo: handle,
      path: path,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<File> createFile(
    String path,
  ) async {
    final request = RequestRepositoryCreateFile(
      repo: handle,
      path: path,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseFile(value: final value): return File(client, value);
      default: throw UnexpectedResponse();
    }
  }

  Future<void> createMirror(
    String host,
  ) async {
    final request = RequestRepositoryCreateMirror(
      repo: handle,
      host: host,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  /// Delete a repository
  Future<void> delete(
  ) async {
    final request = RequestRepositoryDelete(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> deleteMirror(
    String host,
  ) async {
    final request = RequestRepositoryDeleteMirror(
      repo: handle,
      host: host,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  /// Export repository to file
  Future<String> export(
    String outputPath,
  ) async {
    final request = RequestRepositoryExport(
      repo: handle,
      outputPath: outputPath,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponsePath(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<bool> fileExists(
    String path,
  ) async {
    final request = RequestRepositoryFileExists(
      repo: handle,
      path: path,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<AccessMode> getAccessMode(
  ) async {
    final request = RequestRepositoryGetAccessMode(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseAccessMode(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<Duration?> getBlockExpiration(
  ) async {
    final request = RequestRepositoryGetBlockExpiration(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseDuration(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  Future<List<int>> getCredentials(
  ) async {
    final request = RequestRepositoryGetCredentials(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseBytes(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  /// Returns the type of repository entry (file, directory, ...) or `None` if the entry doesn't
  /// exist.
  Future<EntryType?> getEntryType(
    String path,
  ) async {
    final request = RequestRepositoryGetEntryType(
      repo: handle,
      path: path,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseEntryType(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  Future<Duration?> getExpiration(
  ) async {
    final request = RequestRepositoryGetExpiration(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseDuration(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  /// Return the info-hash of the repository formatted as hex string. This can be used as a globally
  /// unique, non-secret identifier of the repository.
  Future<String> getInfoHash(
  ) async {
    final request = RequestRepositoryGetInfoHash(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseString(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<String?> getMetadata(
    String key,
  ) async {
    final request = RequestRepositoryGetMetadata(
      repo: handle,
      key: key,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseString(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  Future<String?> getMountPoint(
  ) async {
    final request = RequestRepositoryGetMountPoint(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponsePath(value: final value): return value;
      case ResponseNone(): return null;
      default: throw UnexpectedResponse();
    }
  }

  Future<String> getPath(
  ) async {
    final request = RequestRepositoryGetPath(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponsePath(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<QuotaInfo> getQuota(
  ) async {
    final request = RequestRepositoryGetQuota(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseQuotaInfo(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<Stats> getStats(
  ) async {
    final request = RequestRepositoryGetStats(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseStats(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<Progress> getSyncProgress(
  ) async {
    final request = RequestRepositoryGetSyncProgress(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseProgress(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<bool> isDhtEnabled(
  ) async {
    final request = RequestRepositoryIsDhtEnabled(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<bool> isPexEnabled(
  ) async {
    final request = RequestRepositoryIsPexEnabled(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<bool> isSyncEnabled(
  ) async {
    final request = RequestRepositoryIsSyncEnabled(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<bool> mirrorExists(
    String host,
  ) async {
    final request = RequestRepositoryMirrorExists(
      repo: handle,
      host: host,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<String> mount(
  ) async {
    final request = RequestRepositoryMount(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponsePath(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> move(
    String dst,
  ) async {
    final request = RequestRepositoryMove(
      repo: handle,
      dst: dst,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> moveEntry(
    String src,
    String dst,
  ) async {
    final request = RequestRepositoryMoveEntry(
      repo: handle,
      src: src,
      dst: dst,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<File> openFile(
    String path,
  ) async {
    final request = RequestRepositoryOpenFile(
      repo: handle,
      path: path,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseFile(value: final value): return File(client, value);
      default: throw UnexpectedResponse();
    }
  }

  Future<List<DirectoryEntry>> readDirectory(
    String path,
  ) async {
    final request = RequestRepositoryReadDirectory(
      repo: handle,
      path: path,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseDirectoryEntries(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  /// Removes the directory at the given path from the repository. If `recursive` is true it removes
  /// also the contents, otherwise the directory must be empty.
  Future<void> removeDirectory(
    String path,
    bool recursive,
  ) async {
    final request = RequestRepositoryRemoveDirectory(
      repo: handle,
      path: path,
      recursive: recursive,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  /// Remove (delete) the file at the given path from the repository.
  Future<void> removeFile(
    String path,
  ) async {
    final request = RequestRepositoryRemoveFile(
      repo: handle,
      path: path,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> resetAccess(
    ShareToken token,
  ) async {
    final request = RequestRepositoryResetAccess(
      repo: handle,
      token: token,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setAccess({
    AccessChange? read,
    AccessChange? write,
  }) async {
    final request = RequestRepositorySetAccess(
      repo: handle,
      read: read,
      write: write,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setAccessMode(
    AccessMode accessMode,
    LocalSecret? localSecret,
  ) async {
    final request = RequestRepositorySetAccessMode(
      repo: handle,
      accessMode: accessMode,
      localSecret: localSecret,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setBlockExpiration(
    Duration? value,
  ) async {
    final request = RequestRepositorySetBlockExpiration(
      repo: handle,
      value: value,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setCredentials(
    List<int> credentials,
  ) async {
    final request = RequestRepositorySetCredentials(
      repo: handle,
      credentials: credentials,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setDhtEnabled(
    bool enabled,
  ) async {
    final request = RequestRepositorySetDhtEnabled(
      repo: handle,
      enabled: enabled,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setExpiration(
    Duration? value,
  ) async {
    final request = RequestRepositorySetExpiration(
      repo: handle,
      value: value,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<bool> setMetadata(
    List<MetadataEdit> edits,
  ) async {
    final request = RequestRepositorySetMetadata(
      repo: handle,
      edits: edits,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setPexEnabled(
    bool enabled,
  ) async {
    final request = RequestRepositorySetPexEnabled(
      repo: handle,
      enabled: enabled,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setQuota(
    StorageSize? value,
  ) async {
    final request = RequestRepositorySetQuota(
      repo: handle,
      value: value,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> setSyncEnabled(
    bool enabled,
  ) async {
    final request = RequestRepositorySetSyncEnabled(
      repo: handle,
      enabled: enabled,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<ShareToken> share({
    LocalSecret? localSecret,
    required AccessMode accessMode,
  }) async {
    final request = RequestRepositoryShare(
      repo: handle,
      localSecret: localSecret,
      accessMode: accessMode,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseShareToken(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> unmount(
  ) async {
    final request = RequestRepositoryUnmount(
      repo: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  @override
  bool operator ==(Object other) =>
      other is Repository &&
      other.client == client &&
      other.handle == handle;

  @override
  int get hashCode => Object.hash(client, handle);

  @override
  String toString() => '$runtimeType($handle)';

}

class File {
  final Client client;
  final FileHandle handle;

  File(this.client, this.handle);

  Future<void> close(
  ) async {
    final request = RequestFileClose(
      file: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> flush(
  ) async {
    final request = RequestFileFlush(
      file: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<int> getLength(
  ) async {
    final request = RequestFileGetLength(
      file: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseU64(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  /// Returns sync progress of the given file.
  Future<int> getProgress(
  ) async {
    final request = RequestFileGetProgress(
      file: handle,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseU64(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  /// Reads `size` bytes from the file starting at `offset` bytes from the beginning of the file.
  Future<List<int>> read(
    int offset,
    int size,
  ) async {
    final request = RequestFileRead(
      file: handle,
      offset: offset,
      size: size,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseBytes(value: final value): return value;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> truncate(
    int len,
  ) async {
    final request = RequestFileTruncate(
      file: handle,
      len: len,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  Future<void> write(
    int offset,
    List<int> data,
  ) async {
    final request = RequestFileWrite(
      file: handle,
      offset: offset,
      data: data,
    );
    final response = await client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw UnexpectedResponse();
    }
  }

  @override
  bool operator ==(Object other) =>
      other is File &&
      other.client == client &&
      other.handle == handle;

  @override
  int get hashCode => Object.hash(client, handle);

  @override
  String toString() => '$runtimeType($handle)';

}

