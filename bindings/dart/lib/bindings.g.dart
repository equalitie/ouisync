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

/// Symmetric encryption/decryption secret key.
///
/// Note: this implementation tries to prevent certain types of attacks by making sure the
/// underlying sensitive key material is always stored at most in one place. This is achieved by
/// putting it on the heap which means it is not moved when the key itself is moved which could
/// otherwise leave a copy of the data in memory. Additionally, the data is behind a `Arc` which
/// means the key can be cheaply cloned without actually cloning the data. Finally, the data is
/// scrambled (overwritten with zeros) when the key is dropped to make sure it does not stay in
/// the memory past its lifetime.
extension type SecretKey(List<int> value) {
  void encode(Packer p) {
    p.packBinary(value);
  }
  
  static SecretKey? decode(Unpacker u) {
    return SecretKey(u.unpackBinary());
  }
}

/// A simple wrapper over String to avoid certain kinds of attack. For more elaboration please see
/// the documentation for the SecretKey structure.
extension type Password(String value) {
  void encode(Packer p) {
    p.packString(value);
  }
  
  static Password? decode(Unpacker u) {
    final value = u.unpackString();
    return value != null ? Password(value) : null;
  }
}

extension type PasswordSalt(List<int> value) {
  void encode(Packer p) {
    p.packBinary(value);
  }
  
  static PasswordSalt? decode(Unpacker u) {
    return PasswordSalt(u.unpackBinary());
  }
}

/// Strongly typed storage size.
extension type StorageSize(int value) {
  void encode(Packer p) {
    p.packInt(value);
  }
  
  static StorageSize? decode(Unpacker u) {
    final value = u.unpackInt();
    return value != null ? StorageSize(value) : null;
  }
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

  static AccessMode? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    switch (this) {
      case AccessMode.blind: p.packInt(0);
      case AccessMode.read: p.packInt(1);
      case AccessMode.write: p.packInt(2);
    }
  }

}

sealed class LocalSecret {
  void encode(Packer p) {
    switch (this) {
      case LocalSecretPassword(
        value: final value,
      ):
        p.packListLength(1);
        value.encode(p);
      case LocalSecretSecretKey(
        value: final value,
      ):
        p.packListLength(1);
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
          if (u.unpackListLength() != 1) throw DecodeError();
          return LocalSecretPassword(
            value: (Password.decode(u))!,
          );
        case "SecretKey":
          if (u.unpackListLength() != 1) throw DecodeError();
          return LocalSecretSecretKey(
            value: (SecretKey.decode(u))!,
          );
        case null: return null;
        default: throw DecodeError();
      }
    }
  }
  
}

class LocalSecretPassword extends LocalSecret {
  final Password value;

  LocalSecretPassword({
    required this.value,
  });
}

class LocalSecretSecretKey extends LocalSecret {
  final SecretKey value;

  LocalSecretSecretKey({
    required this.value,
  });
}

sealed class SetLocalSecret {
  void encode(Packer p) {
    switch (this) {
      case SetLocalSecretPassword(
        value: final value,
      ):
        p.packListLength(1);
        value.encode(p);
      case SetLocalSecretKeyAndSalt(
        value: final value,
      ):
        p.packListLength(1);
        value.encode(p);
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
          if (u.unpackListLength() != 1) throw DecodeError();
          return SetLocalSecretPassword(
            value: (Password.decode(u))!,
          );
        case "KeyAndSalt":
          if (u.unpackListLength() != 1) throw DecodeError();
          return SetLocalSecretKeyAndSalt(
            value: (KeyAndSalt.decode(u))!,
          );
        case null: return null;
        default: throw DecodeError();
      }
    }
  }
  
}

class SetLocalSecretPassword extends SetLocalSecret {
  final Password value;

  SetLocalSecretPassword({
    required this.value,
  });
}

class SetLocalSecretKeyAndSalt extends SetLocalSecret {
  final KeyAndSalt value;

  SetLocalSecretKeyAndSalt({
    required this.value,
  });
}

class KeyAndSalt {
  final SecretKey key;
  final PasswordSalt salt;

  KeyAndSalt({
    required this.key,
    required this.salt,
  });
  
  void encode(Packer p) {
    p.packListLength(2);
    key.encode(p);
    salt.encode(p);
  }
  
  static KeyAndSalt? decode(Unpacker u) {
    switch (u.unpackListLength()) {
      case 0: return null;
      case 2: break;
      default: throw DecodeError();
    }
    return KeyAndSalt(
      key: (SecretKey.decode(u))!,
      salt: (PasswordSalt.decode(u))!,
    );
  }
}

sealed class AccessChange {
  void encode(Packer p) {
    switch (this) {
      case AccessChangeEnable(
        value: final value,
      ):
        p.packListLength(1);
        _encodeNullable(p, value, (p, e) => e.encode(p));
      case AccessChangeDisable(
      ):
        p.packListLength(0);
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
          if (u.unpackListLength() != 1) throw DecodeError();
          return AccessChangeEnable(
            value: SetLocalSecret.decode(u),
          );
        case null: return null;
        default: throw DecodeError();
      }
    }
  }
  
}

class AccessChangeEnable extends AccessChange {
  final SetLocalSecret? value;

  AccessChangeEnable({
    required this.value,
  });
}

class AccessChangeDisable extends AccessChange {
  AccessChangeDisable(
  );
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

  static EntryType? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    switch (this) {
      case EntryType.file: p.packInt(1);
      case EntryType.directory: p.packInt(2);
    }
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

  static NetworkEvent? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    switch (this) {
      case NetworkEvent.protocolVersionMismatch: p.packInt(0);
      case NetworkEvent.peerSetChange: p.packInt(1);
    }
  }

}

/// Information about a peer.
class PeerInfo {
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

  static PeerSource? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    switch (this) {
      case PeerSource.userProvided: p.packInt(0);
      case PeerSource.listener: p.packInt(1);
      case PeerSource.localDiscovery: p.packInt(2);
      case PeerSource.dht: p.packInt(3);
      case PeerSource.peerExchange: p.packInt(4);
    }
  }

}

sealed class PeerState {
  void encode(Packer p) {
    switch (this) {
      case PeerStateKnown(
      ):
        p.packListLength(0);
      case PeerStateConnecting(
      ):
        p.packListLength(0);
      case PeerStateHandshaking(
      ):
        p.packListLength(0);
      case PeerStateActive(
        id: final id,
        since: final since,
      ):
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
  PeerStateKnown(
  );
}

class PeerStateConnecting extends PeerState {
  PeerStateConnecting(
  );
}

class PeerStateHandshaking extends PeerState {
  PeerStateHandshaking(
  );
}

class PeerStateActive extends PeerState {
  final PublicRuntimeId id;
  final DateTime since;

  PeerStateActive({
    required this.id,
    required this.since,
  });
}

extension type PublicRuntimeId(List<int> value) {
  void encode(Packer p) {
    p.packBinary(value);
  }
  
  static PublicRuntimeId? decode(Unpacker u) {
    return PublicRuntimeId(u.unpackBinary());
  }
}

/// Network traffic statistics.
class Stats {
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
}

/// Progress of a task.
class Progress {
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

  static NatBehavior? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    switch (this) {
      case NatBehavior.endpointIndependent: p.packInt(0);
      case NatBehavior.addressDependent: p.packInt(1);
      case NatBehavior.addressAndPortDependent: p.packInt(2);
    }
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

  static ErrorCode? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    switch (this) {
      case ErrorCode.ok: p.packInt(0);
      case ErrorCode.permissionDenied: p.packInt(1);
      case ErrorCode.invalidInput: p.packInt(2);
      case ErrorCode.invalidData: p.packInt(3);
      case ErrorCode.alreadyExists: p.packInt(4);
      case ErrorCode.notFound: p.packInt(5);
      case ErrorCode.ambiguous: p.packInt(6);
      case ErrorCode.unsupported: p.packInt(8);
      case ErrorCode.connectionRefused: p.packInt(1025);
      case ErrorCode.connectionAborted: p.packInt(1026);
      case ErrorCode.transportError: p.packInt(1027);
      case ErrorCode.listenerBindError: p.packInt(1028);
      case ErrorCode.listenerAcceptError: p.packInt(1029);
      case ErrorCode.storeError: p.packInt(2049);
      case ErrorCode.isDirectory: p.packInt(2050);
      case ErrorCode.notDirectory: p.packInt(2051);
      case ErrorCode.directoryNotEmpty: p.packInt(2052);
      case ErrorCode.resourceBusy: p.packInt(2053);
      case ErrorCode.runtimeInitializeError: p.packInt(4097);
      case ErrorCode.loggerInitializeError: p.packInt(4098);
      case ErrorCode.configError: p.packInt(4099);
      case ErrorCode.tlsCertificatesNotFound: p.packInt(4100);
      case ErrorCode.tlsCertificatesInvalid: p.packInt(4101);
      case ErrorCode.tlsKeysNotFound: p.packInt(4102);
      case ErrorCode.tlsConfigError: p.packInt(4103);
      case ErrorCode.vfsDriverInstallError: p.packInt(4104);
      case ErrorCode.vfsOtherError: p.packInt(4105);
      case ErrorCode.serviceAlreadyRunning: p.packInt(4106);
      case ErrorCode.storeDirUnspecified: p.packInt(4107);
      case ErrorCode.other: p.packInt(65535);
    }
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

  static LogLevel? decode(Unpacker u) {
    final n = u.unpackInt();
    return n != null ? fromInt(n) : null;
  }

  void encode(Packer p) {
    switch (this) {
      case LogLevel.error: p.packInt(1);
      case LogLevel.warn: p.packInt(2);
      case LogLevel.info: p.packInt(3);
      case LogLevel.debug: p.packInt(4);
      case LogLevel.trace: p.packInt(5);
    }
  }

}

extension type MessageId(int value) {
  void encode(Packer p) {
    p.packInt(value);
  }
  
  static MessageId? decode(Unpacker u) {
    final value = u.unpackInt();
    return value != null ? MessageId(value) : null;
  }
}

/// Edit of a single metadata entry.
class MetadataEdit {
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
}

class NetworkDefaults {
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
}

class DirectoryEntry {
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
}

class QuotaInfo {
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
}

extension type FileHandle(int value) {
  void encode(Packer p) {
    p.packInt(value);
  }
  
  static FileHandle? decode(Unpacker u) {
    final value = u.unpackInt();
    return value != null ? FileHandle(value) : null;
  }
}

extension type RepositoryHandle(int value) {
  void encode(Packer p) {
    p.packInt(value);
  }
  
  static RepositoryHandle? decode(Unpacker u) {
    final value = u.unpackInt();
    return value != null ? RepositoryHandle(value) : null;
  }
}

sealed class Request {
  void encode(Packer p) {
    switch (this) {
      case RequestFileClose(
        file: final file,
      ):
        p.packListLength(1);
        file.encode(p);
      case RequestFileFlush(
        file: final file,
      ):
        p.packListLength(1);
        file.encode(p);
      case RequestFileLen(
        file: final file,
      ):
        p.packListLength(1);
        file.encode(p);
      case RequestFileProgress(
        file: final file,
      ):
        p.packListLength(1);
        file.encode(p);
      case RequestFileRead(
        file: final file,
        offset: final offset,
        len: final len,
      ):
        p.packListLength(3);
        file.encode(p);
        p.packInt(offset);
        p.packInt(len);
      case RequestFileTruncate(
        file: final file,
        len: final len,
      ):
        p.packListLength(2);
        file.encode(p);
        p.packInt(len);
      case RequestFileWrite(
        file: final file,
        offset: final offset,
        data: final data,
      ):
        p.packListLength(3);
        file.encode(p);
        p.packInt(offset);
        p.packBinary(data);
      case RequestMetricsBind(
        addr: final addr,
      ):
        p.packListLength(1);
        _encodeNullable(p, addr, (p, e) => p.packString(e));
      case RequestMetricsGetListenerAddr(
      ):
        p.packListLength(0);
      case RequestRemoteControlBind(
        addr: final addr,
      ):
        p.packListLength(1);
        _encodeNullable(p, addr, (p, e) => p.packString(e));
      case RequestRemoteControlGetListenerAddr(
      ):
        p.packListLength(0);
      case RequestRepositoryClose(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryCreateDirectory(
        repo: final repo,
        path: final path,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryCreateFile(
        repo: final repo,
        path: final path,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryCreateMirror(
        repo: final repo,
        host: final host,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(host);
      case RequestRepositoryDelete(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryDeleteMirror(
        repo: final repo,
        host: final host,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(host);
      case RequestRepositoryExport(
        repo: final repo,
        outputPath: final outputPath,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(outputPath);
      case RequestRepositoryFileExists(
        repo: final repo,
        path: final path,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryGetAccessMode(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetBlockExpiration(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetCredentials(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetEntryType(
        repo: final repo,
        path: final path,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryGetExpiration(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetInfoHash(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetMetadata(
        repo: final repo,
        key: final key,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(key);
      case RequestRepositoryGetMountPoint(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetPath(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetQuota(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetStats(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryGetSyncProgress(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryIsDhtEnabled(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryIsPexEnabled(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryIsSyncEnabled(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryMirrorExists(
        repo: final repo,
        host: final host,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(host);
      case RequestRepositoryMount(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryMove(
        repo: final repo,
        dst: final dst,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(dst);
      case RequestRepositoryMoveEntry(
        repo: final repo,
        src: final src,
        dst: final dst,
      ):
        p.packListLength(3);
        repo.encode(p);
        p.packString(src);
        p.packString(dst);
      case RequestRepositoryOpenFile(
        repo: final repo,
        path: final path,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryReadDirectory(
        repo: final repo,
        path: final path,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryRemoveDirectory(
        repo: final repo,
        path: final path,
        recursive: final recursive,
      ):
        p.packListLength(3);
        repo.encode(p);
        p.packString(path);
        p.packBool(recursive);
      case RequestRepositoryRemoveFile(
        repo: final repo,
        path: final path,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(path);
      case RequestRepositoryResetAccess(
        repo: final repo,
        token: final token,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packString(token);
      case RequestRepositorySetAccess(
        repo: final repo,
        read: final read,
        write: final write,
      ):
        p.packListLength(3);
        repo.encode(p);
        _encodeNullable(p, read, (p, e) => e.encode(p));
        _encodeNullable(p, write, (p, e) => e.encode(p));
      case RequestRepositorySetAccessMode(
        repo: final repo,
        accessMode: final accessMode,
        localSecret: final localSecret,
      ):
        p.packListLength(3);
        repo.encode(p);
        accessMode.encode(p);
        _encodeNullable(p, localSecret, (p, e) => e.encode(p));
      case RequestRepositorySetBlockExpiration(
        repo: final repo,
        value: final value,
      ):
        p.packListLength(2);
        repo.encode(p);
        _encodeNullable(p, value, (p, e) => _encodeDuration(p, e));
      case RequestRepositorySetCredentials(
        repo: final repo,
        credentials: final credentials,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packBinary(credentials);
      case RequestRepositorySetDhtEnabled(
        repo: final repo,
        enabled: final enabled,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packBool(enabled);
      case RequestRepositorySetExpiration(
        repo: final repo,
        value: final value,
      ):
        p.packListLength(2);
        repo.encode(p);
        _encodeNullable(p, value, (p, e) => _encodeDuration(p, e));
      case RequestRepositorySetMetadata(
        repo: final repo,
        edits: final edits,
      ):
        p.packListLength(2);
        repo.encode(p);
        _encodeList(p, edits, (p, e) => e.encode(p));
      case RequestRepositorySetPexEnabled(
        repo: final repo,
        enabled: final enabled,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packBool(enabled);
      case RequestRepositorySetQuota(
        repo: final repo,
        value: final value,
      ):
        p.packListLength(2);
        repo.encode(p);
        _encodeNullable(p, value, (p, e) => e.encode(p));
      case RequestRepositorySetSyncEnabled(
        repo: final repo,
        enabled: final enabled,
      ):
        p.packListLength(2);
        repo.encode(p);
        p.packBool(enabled);
      case RequestRepositoryShare(
        repo: final repo,
        localSecret: final localSecret,
        accessMode: final accessMode,
      ):
        p.packListLength(3);
        repo.encode(p);
        _encodeNullable(p, localSecret, (p, e) => e.encode(p));
        accessMode.encode(p);
      case RequestRepositorySubscribe(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestRepositoryUnmount(
        repo: final repo,
      ):
        p.packListLength(1);
        repo.encode(p);
      case RequestSessionAddUserProvidedPeers(
        addrs: final addrs,
      ):
        p.packListLength(1);
        _encodeList(p, addrs, (p, e) => p.packString(e));
      case RequestSessionBindNetwork(
        addrs: final addrs,
      ):
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
        p.packListLength(7);
        p.packString(path);
        _encodeNullable(p, readSecret, (p, e) => e.encode(p));
        _encodeNullable(p, writeSecret, (p, e) => e.encode(p));
        _encodeNullable(p, token, (p, e) => p.packString(e));
        p.packBool(syncEnabled);
        p.packBool(dhtEnabled);
        p.packBool(pexEnabled);
      case RequestSessionDeleteRepositoryByName(
        name: final name,
      ):
        p.packListLength(1);
        p.packString(name);
      case RequestSessionDeriveSecretKey(
        password: final password,
        salt: final salt,
      ):
        p.packListLength(2);
        p.packString(password);
        salt.encode(p);
      case RequestSessionFindRepository(
        name: final name,
      ):
        p.packListLength(1);
        p.packString(name);
      case RequestSessionGeneratePasswordSalt(
      ):
        p.packListLength(0);
      case RequestSessionGetCurrentProtocolVersion(
      ):
        p.packListLength(0);
      case RequestSessionGetDefaultBlockExpiration(
      ):
        p.packListLength(0);
      case RequestSessionGetDefaultQuota(
      ):
        p.packListLength(0);
      case RequestSessionGetDefaultRepositoryExpiration(
      ):
        p.packListLength(0);
      case RequestSessionGetExternalAddrV4(
      ):
        p.packListLength(0);
      case RequestSessionGetExternalAddrV6(
      ):
        p.packListLength(0);
      case RequestSessionGetHighestSeenProtocolVersion(
      ):
        p.packListLength(0);
      case RequestSessionGetLocalListenerAddrs(
      ):
        p.packListLength(0);
      case RequestSessionGetMountRoot(
      ):
        p.packListLength(0);
      case RequestSessionGetNatBehavior(
      ):
        p.packListLength(0);
      case RequestSessionGetNetworkStats(
      ):
        p.packListLength(0);
      case RequestSessionGetPeers(
      ):
        p.packListLength(0);
      case RequestSessionGetRemoteListenerAddrs(
        host: final host,
      ):
        p.packListLength(1);
        p.packString(host);
      case RequestSessionGetRuntimeId(
      ):
        p.packListLength(0);
      case RequestSessionGetStateMonitor(
        path: final path,
      ):
        p.packListLength(1);
        _encodeList(p, path, (p, e) => e.encode(p));
      case RequestSessionGetStoreDir(
      ):
        p.packListLength(0);
      case RequestSessionGetUserProvidedPeers(
      ):
        p.packListLength(0);
      case RequestSessionInitNetwork(
        defaults: final defaults,
      ):
        p.packListLength(1);
        defaults.encode(p);
      case RequestSessionIsLocalDiscoveryEnabled(
      ):
        p.packListLength(0);
      case RequestSessionIsPexRecvEnabled(
      ):
        p.packListLength(0);
      case RequestSessionIsPexSendEnabled(
      ):
        p.packListLength(0);
      case RequestSessionIsPortForwardingEnabled(
      ):
        p.packListLength(0);
      case RequestSessionListRepositories(
      ):
        p.packListLength(0);
      case RequestSessionOpenRepository(
        path: final path,
        localSecret: final localSecret,
      ):
        p.packListLength(2);
        p.packString(path);
        _encodeNullable(p, localSecret, (p, e) => e.encode(p));
      case RequestSessionRemoveUserProvidedPeers(
        addrs: final addrs,
      ):
        p.packListLength(1);
        _encodeList(p, addrs, (p, e) => p.packString(e));
      case RequestSessionSetDefaultBlockExpiration(
        value: final value,
      ):
        p.packListLength(1);
        _encodeNullable(p, value, (p, e) => _encodeDuration(p, e));
      case RequestSessionSetDefaultQuota(
        value: final value,
      ):
        p.packListLength(1);
        _encodeNullable(p, value, (p, e) => e.encode(p));
      case RequestSessionSetDefaultRepositoryExpiration(
        value: final value,
      ):
        p.packListLength(1);
        _encodeNullable(p, value, (p, e) => _encodeDuration(p, e));
      case RequestSessionSetLocalDiscoveryEnabled(
        enabled: final enabled,
      ):
        p.packListLength(1);
        p.packBool(enabled);
      case RequestSessionSetMountRoot(
        path: final path,
      ):
        p.packListLength(1);
        _encodeNullable(p, path, (p, e) => p.packString(e));
      case RequestSessionSetPexRecvEnabled(
        enabled: final enabled,
      ):
        p.packListLength(1);
        p.packBool(enabled);
      case RequestSessionSetPexSendEnabled(
        enabled: final enabled,
      ):
        p.packListLength(1);
        p.packBool(enabled);
      case RequestSessionSetPortForwardingEnabled(
        enabled: final enabled,
      ):
        p.packListLength(1);
        p.packBool(enabled);
      case RequestSessionSetStoreDir(
        path: final path,
      ):
        p.packListLength(1);
        p.packString(path);
      case RequestSessionSubscribeToNetwork(
      ):
        p.packListLength(0);
      case RequestSessionSubscribeToStateMonitor(
        path: final path,
      ):
        p.packListLength(1);
        _encodeList(p, path, (p, e) => e.encode(p));
      case RequestSessionUnsubscribe(
        id: final id,
      ):
        p.packListLength(1);
        id.encode(p);
      case RequestShareTokenGetAccessMode(
        token: final token,
      ):
        p.packListLength(1);
        p.packString(token);
      case RequestShareTokenGetInfoHash(
        token: final token,
      ):
        p.packListLength(1);
        p.packString(token);
      case RequestShareTokenGetSuggestedName(
        token: final token,
      ):
        p.packListLength(1);
        p.packString(token);
      case RequestShareTokenMirrorExists(
        token: final token,
        host: final host,
      ):
        p.packListLength(2);
        p.packString(token);
        p.packString(host);
      case RequestShareTokenNormalize(
        token: final token,
      ):
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

class RequestFileLen extends Request {
  final FileHandle file;

  RequestFileLen({
    required this.file,
  });
}

class RequestFileProgress extends Request {
  final FileHandle file;

  RequestFileProgress({
    required this.file,
  });
}

class RequestFileRead extends Request {
  final FileHandle file;
  final int offset;
  final int len;

  RequestFileRead({
    required this.file,
    required this.offset,
    required this.len,
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
  RequestMetricsGetListenerAddr(
  );
}

class RequestRemoteControlBind extends Request {
  final String? addr;

  RequestRemoteControlBind({
    required this.addr,
  });
}

class RequestRemoteControlGetListenerAddr extends Request {
  RequestRemoteControlGetListenerAddr(
  );
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
  final String token;

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
  final String? token;
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
  final String password;
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
  RequestSessionGeneratePasswordSalt(
  );
}

class RequestSessionGetCurrentProtocolVersion extends Request {
  RequestSessionGetCurrentProtocolVersion(
  );
}

class RequestSessionGetDefaultBlockExpiration extends Request {
  RequestSessionGetDefaultBlockExpiration(
  );
}

class RequestSessionGetDefaultQuota extends Request {
  RequestSessionGetDefaultQuota(
  );
}

class RequestSessionGetDefaultRepositoryExpiration extends Request {
  RequestSessionGetDefaultRepositoryExpiration(
  );
}

class RequestSessionGetExternalAddrV4 extends Request {
  RequestSessionGetExternalAddrV4(
  );
}

class RequestSessionGetExternalAddrV6 extends Request {
  RequestSessionGetExternalAddrV6(
  );
}

class RequestSessionGetHighestSeenProtocolVersion extends Request {
  RequestSessionGetHighestSeenProtocolVersion(
  );
}

class RequestSessionGetLocalListenerAddrs extends Request {
  RequestSessionGetLocalListenerAddrs(
  );
}

class RequestSessionGetMountRoot extends Request {
  RequestSessionGetMountRoot(
  );
}

class RequestSessionGetNatBehavior extends Request {
  RequestSessionGetNatBehavior(
  );
}

class RequestSessionGetNetworkStats extends Request {
  RequestSessionGetNetworkStats(
  );
}

class RequestSessionGetPeers extends Request {
  RequestSessionGetPeers(
  );
}

class RequestSessionGetRemoteListenerAddrs extends Request {
  final String host;

  RequestSessionGetRemoteListenerAddrs({
    required this.host,
  });
}

class RequestSessionGetRuntimeId extends Request {
  RequestSessionGetRuntimeId(
  );
}

class RequestSessionGetStateMonitor extends Request {
  final List<MonitorId> path;

  RequestSessionGetStateMonitor({
    required this.path,
  });
}

class RequestSessionGetStoreDir extends Request {
  RequestSessionGetStoreDir(
  );
}

class RequestSessionGetUserProvidedPeers extends Request {
  RequestSessionGetUserProvidedPeers(
  );
}

class RequestSessionInitNetwork extends Request {
  final NetworkDefaults defaults;

  RequestSessionInitNetwork({
    required this.defaults,
  });
}

class RequestSessionIsLocalDiscoveryEnabled extends Request {
  RequestSessionIsLocalDiscoveryEnabled(
  );
}

class RequestSessionIsPexRecvEnabled extends Request {
  RequestSessionIsPexRecvEnabled(
  );
}

class RequestSessionIsPexSendEnabled extends Request {
  RequestSessionIsPexSendEnabled(
  );
}

class RequestSessionIsPortForwardingEnabled extends Request {
  RequestSessionIsPortForwardingEnabled(
  );
}

class RequestSessionListRepositories extends Request {
  RequestSessionListRepositories(
  );
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
  RequestSessionSubscribeToNetwork(
  );
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

class RequestShareTokenGetAccessMode extends Request {
  final String token;

  RequestShareTokenGetAccessMode({
    required this.token,
  });
}

class RequestShareTokenGetInfoHash extends Request {
  final String token;

  RequestShareTokenGetInfoHash({
    required this.token,
  });
}

class RequestShareTokenGetSuggestedName extends Request {
  final String token;

  RequestShareTokenGetSuggestedName({
    required this.token,
  });
}

class RequestShareTokenMirrorExists extends Request {
  final String token;
  final String host;

  RequestShareTokenMirrorExists({
    required this.token,
    required this.host,
  });
}

class RequestShareTokenNormalize extends Request {
  final String token;

  RequestShareTokenNormalize({
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
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseAccessMode(
            value: (AccessMode.decode(u))!,
          );
        case "Bool":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseBool(
            value: (u.unpackBool())!,
          );
        case "Bytes":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseBytes(
            value: u.unpackBinary(),
          );
        case "DirectoryEntries":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseDirectoryEntries(
            value: _decodeList(u, (u) => (DirectoryEntry.decode(u))!),
          );
        case "Duration":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseDuration(
            value: (_decodeDuration(u))!,
          );
        case "EntryType":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseEntryType(
            value: (EntryType.decode(u))!,
          );
        case "File":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseFile(
            value: (FileHandle.decode(u))!,
          );
        case "NatBehavior":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseNatBehavior(
            value: (NatBehavior.decode(u))!,
          );
        case "NetworkEvent":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseNetworkEvent(
            value: (NetworkEvent.decode(u))!,
          );
        case "PasswordSalt":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponsePasswordSalt(
            value: (PasswordSalt.decode(u))!,
          );
        case "Path":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponsePath(
            value: (u.unpackString())!,
          );
        case "PeerAddrs":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponsePeerAddrs(
            value: _decodeList(u, (u) => (u.unpackString())!),
          );
        case "PeerInfos":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponsePeerInfos(
            value: _decodeList(u, (u) => (PeerInfo.decode(u))!),
          );
        case "Progress":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseProgress(
            value: (Progress.decode(u))!,
          );
        case "PublicRuntimeId":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponsePublicRuntimeId(
            value: (PublicRuntimeId.decode(u))!,
          );
        case "QuotaInfo":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseQuotaInfo(
            value: (QuotaInfo.decode(u))!,
          );
        case "Repositories":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseRepositories(
            value: _decodeMap(u, (u) => (u.unpackString())!, (u) => (RepositoryHandle.decode(u))!),
          );
        case "Repository":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseRepository(
            value: (RepositoryHandle.decode(u))!,
          );
        case "SecretKey":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseSecretKey(
            value: (SecretKey.decode(u))!,
          );
        case "ShareToken":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseShareToken(
            value: (u.unpackString())!,
          );
        case "SocketAddr":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseSocketAddr(
            value: (u.unpackString())!,
          );
        case "StateMonitor":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseStateMonitor(
            value: (StateMonitorNode.decode(u))!,
          );
        case "Stats":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseStats(
            value: (Stats.decode(u))!,
          );
        case "StorageSize":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseStorageSize(
            value: (StorageSize.decode(u))!,
          );
        case "String":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseString(
            value: (u.unpackString())!,
          );
        case "U16":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseU16(
            value: (u.unpackInt())!,
          );
        case "U64":
          if (u.unpackListLength() != 1) throw DecodeError();
          return ResponseU64(
            value: (u.unpackInt())!,
          );
        case null: return null;
        default: throw DecodeError();
      }
    }
  }
  
}

class ResponseAccessMode extends Response {
  final AccessMode value;

  ResponseAccessMode({
    required this.value,
  });
}

class ResponseBool extends Response {
  final bool value;

  ResponseBool({
    required this.value,
  });
}

class ResponseBytes extends Response {
  final List<int> value;

  ResponseBytes({
    required this.value,
  });
}

class ResponseDirectoryEntries extends Response {
  final List<DirectoryEntry> value;

  ResponseDirectoryEntries({
    required this.value,
  });
}

class ResponseDuration extends Response {
  final Duration value;

  ResponseDuration({
    required this.value,
  });
}

class ResponseEntryType extends Response {
  final EntryType value;

  ResponseEntryType({
    required this.value,
  });
}

class ResponseFile extends Response {
  final FileHandle value;

  ResponseFile({
    required this.value,
  });
}

class ResponseNatBehavior extends Response {
  final NatBehavior value;

  ResponseNatBehavior({
    required this.value,
  });
}

class ResponseNetworkEvent extends Response {
  final NetworkEvent value;

  ResponseNetworkEvent({
    required this.value,
  });
}

class ResponseNone extends Response {
  ResponseNone(
  );
}

class ResponsePasswordSalt extends Response {
  final PasswordSalt value;

  ResponsePasswordSalt({
    required this.value,
  });
}

class ResponsePath extends Response {
  final String value;

  ResponsePath({
    required this.value,
  });
}

class ResponsePeerAddrs extends Response {
  final List<String> value;

  ResponsePeerAddrs({
    required this.value,
  });
}

class ResponsePeerInfos extends Response {
  final List<PeerInfo> value;

  ResponsePeerInfos({
    required this.value,
  });
}

class ResponseProgress extends Response {
  final Progress value;

  ResponseProgress({
    required this.value,
  });
}

class ResponsePublicRuntimeId extends Response {
  final PublicRuntimeId value;

  ResponsePublicRuntimeId({
    required this.value,
  });
}

class ResponseQuotaInfo extends Response {
  final QuotaInfo value;

  ResponseQuotaInfo({
    required this.value,
  });
}

class ResponseRepositories extends Response {
  final Map<String, RepositoryHandle> value;

  ResponseRepositories({
    required this.value,
  });
}

class ResponseRepository extends Response {
  final RepositoryHandle value;

  ResponseRepository({
    required this.value,
  });
}

class ResponseRepositoryEvent extends Response {
  ResponseRepositoryEvent(
  );
}

class ResponseSecretKey extends Response {
  final SecretKey value;

  ResponseSecretKey({
    required this.value,
  });
}

class ResponseShareToken extends Response {
  final String value;

  ResponseShareToken({
    required this.value,
  });
}

class ResponseSocketAddr extends Response {
  final String value;

  ResponseSocketAddr({
    required this.value,
  });
}

class ResponseStateMonitor extends Response {
  final StateMonitorNode value;

  ResponseStateMonitor({
    required this.value,
  });
}

class ResponseStateMonitorEvent extends Response {
  ResponseStateMonitorEvent(
  );
}

class ResponseStats extends Response {
  final Stats value;

  ResponseStats({
    required this.value,
  });
}

class ResponseStorageSize extends Response {
  final StorageSize value;

  ResponseStorageSize({
    required this.value,
  });
}

class ResponseString extends Response {
  final String value;

  ResponseString({
    required this.value,
  });
}

class ResponseU16 extends Response {
  final int value;

  ResponseU16({
    required this.value,
  });
}

class ResponseU64 extends Response {
  final int value;

  ResponseU64({
    required this.value,
  });
}

class Session {
  final Client _client;
  
  Session(this._client);
  
  Future<void> addUserProvidedPeers(
    List<String> addrs,
  ) async {
    final request = RequestSessionAddUserProvidedPeers(
      addrs: addrs,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> bindNetwork(
    List<String> addrs,
  ) async {
    final request = RequestSessionBindNetwork(
      addrs: addrs,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<Repository> createRepository(
    String path,
    SetLocalSecret? readSecret,
    SetLocalSecret? writeSecret,
    String? token,
    bool syncEnabled,
    bool dhtEnabled,
    bool pexEnabled,
  ) async {
    final request = RequestSessionCreateRepository(
      path: path,
      readSecret: readSecret,
      writeSecret: writeSecret,
      token: token,
      syncEnabled: syncEnabled,
      dhtEnabled: dhtEnabled,
      pexEnabled: pexEnabled,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseRepository(value: final value): return Repository(_client, value);
      default: throw InvalidData('unexpected response');
    }
  }
  
  /// Delete a repository with the given name.
  Future<void> deleteRepositoryByName(
    String name,
  ) async {
    final request = RequestSessionDeleteRepositoryByName(
      name: name,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<SecretKey> deriveSecretKey(
    String password,
    PasswordSalt salt,
  ) async {
    final request = RequestSessionDeriveSecretKey(
      password: password,
      salt: salt,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseSecretKey(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<Repository> findRepository(
    String name,
  ) async {
    final request = RequestSessionFindRepository(
      name: name,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseRepository(value: final value): return Repository(_client, value);
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<PasswordSalt> generatePasswordSalt(
  ) async {
    final request = RequestSessionGeneratePasswordSalt(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponsePasswordSalt(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<int> getCurrentProtocolVersion(
  ) async {
    final request = RequestSessionGetCurrentProtocolVersion(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseU64(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<Duration?> getDefaultBlockExpiration(
  ) async {
    final request = RequestSessionGetDefaultBlockExpiration(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseDuration(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<StorageSize?> getDefaultQuota(
  ) async {
    final request = RequestSessionGetDefaultQuota(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseStorageSize(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<Duration?> getDefaultRepositoryExpiration(
  ) async {
    final request = RequestSessionGetDefaultRepositoryExpiration(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseDuration(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<String?> getExternalAddrV4(
  ) async {
    final request = RequestSessionGetExternalAddrV4(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseSocketAddr(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<String?> getExternalAddrV6(
  ) async {
    final request = RequestSessionGetExternalAddrV6(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseSocketAddr(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<int> getHighestSeenProtocolVersion(
  ) async {
    final request = RequestSessionGetHighestSeenProtocolVersion(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseU64(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<List<String>> getLocalListenerAddrs(
  ) async {
    final request = RequestSessionGetLocalListenerAddrs(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponsePeerAddrs(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<String?> getMountRoot(
  ) async {
    final request = RequestSessionGetMountRoot(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponsePath(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<NatBehavior?> getNatBehavior(
  ) async {
    final request = RequestSessionGetNatBehavior(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNatBehavior(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<Stats> getNetworkStats(
  ) async {
    final request = RequestSessionGetNetworkStats(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseStats(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<List<PeerInfo>> getPeers(
  ) async {
    final request = RequestSessionGetPeers(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponsePeerInfos(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<List<String>> getRemoteListenerAddrs(
    String host,
  ) async {
    final request = RequestSessionGetRemoteListenerAddrs(
      host: host,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponsePeerAddrs(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<PublicRuntimeId> getRuntimeId(
  ) async {
    final request = RequestSessionGetRuntimeId(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponsePublicRuntimeId(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<StateMonitorNode?> getStateMonitor(
    List<MonitorId> path,
  ) async {
    final request = RequestSessionGetStateMonitor(
      path: path,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseStateMonitor(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<String?> getStoreDir(
  ) async {
    final request = RequestSessionGetStoreDir(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponsePath(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<List<String>> getUserProvidedPeers(
  ) async {
    final request = RequestSessionGetUserProvidedPeers(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponsePeerAddrs(value: final value): return value;
      default: throw InvalidData('unexpected response');
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
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<bool> isLocalDiscoveryEnabled(
  ) async {
    final request = RequestSessionIsLocalDiscoveryEnabled(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  /// Checks whether accepting peers discovered on the peer exchange is enabled.
  Future<bool> isPexRecvEnabled(
  ) async {
    final request = RequestSessionIsPexRecvEnabled(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<bool> isPexSendEnabled(
  ) async {
    final request = RequestSessionIsPexSendEnabled(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<bool> isPortForwardingEnabled(
  ) async {
    final request = RequestSessionIsPortForwardingEnabled(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<Map<String, RepositoryHandle>> listRepositories(
  ) async {
    final request = RequestSessionListRepositories(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseRepositories(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<Repository> openRepository(
    String path,
    LocalSecret? localSecret,
  ) async {
    final request = RequestSessionOpenRepository(
      path: path,
      localSecret: localSecret,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseRepository(value: final value): return Repository(_client, value);
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> removeUserProvidedPeers(
    List<String> addrs,
  ) async {
    final request = RequestSessionRemoveUserProvidedPeers(
      addrs: addrs,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setDefaultBlockExpiration(
    Duration? value,
  ) async {
    final request = RequestSessionSetDefaultBlockExpiration(
      value: value,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setDefaultQuota(
    StorageSize? value,
  ) async {
    final request = RequestSessionSetDefaultQuota(
      value: value,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setDefaultRepositoryExpiration(
    Duration? value,
  ) async {
    final request = RequestSessionSetDefaultRepositoryExpiration(
      value: value,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setLocalDiscoveryEnabled(
    bool enabled,
  ) async {
    final request = RequestSessionSetLocalDiscoveryEnabled(
      enabled: enabled,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setMountRoot(
    String? path,
  ) async {
    final request = RequestSessionSetMountRoot(
      path: path,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setPexRecvEnabled(
    bool enabled,
  ) async {
    final request = RequestSessionSetPexRecvEnabled(
      enabled: enabled,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setPexSendEnabled(
    bool enabled,
  ) async {
    final request = RequestSessionSetPexSendEnabled(
      enabled: enabled,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setPortForwardingEnabled(
    bool enabled,
  ) async {
    final request = RequestSessionSetPortForwardingEnabled(
      enabled: enabled,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setStoreDir(
    String path,
  ) async {
    final request = RequestSessionSetStoreDir(
      path: path,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> subscribeToNetwork(
  ) async {
    final request = RequestSessionSubscribeToNetwork(
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> subscribeToStateMonitor(
    List<MonitorId> path,
  ) async {
    final request = RequestSessionSubscribeToStateMonitor(
      path: path,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  /// Cancel a subscription identified by the given message id. The message id should be the same
  /// that was used for sending the corresponding subscribe request.
  Future<void> unsubscribe(
    MessageId id,
  ) async {
    final request = RequestSessionUnsubscribe(
      id: id,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> close() async {
    await _client.close();
  }
  
}

class Repository {
  final Client _client;
  final RepositoryHandle _handle;
  
  Repository(this._client, this._handle);
  
  Future<void> close(
  ) async {
    final request = RequestRepositoryClose(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> createDirectory(
    String path,
  ) async {
    final request = RequestRepositoryCreateDirectory(
      repo: _handle,
      path: path,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<File> createFile(
    String path,
  ) async {
    final request = RequestRepositoryCreateFile(
      repo: _handle,
      path: path,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseFile(value: final value): return File(_client, value);
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> createMirror(
    String host,
  ) async {
    final request = RequestRepositoryCreateMirror(
      repo: _handle,
      host: host,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  /// Delete a repository
  Future<void> delete(
  ) async {
    final request = RequestRepositoryDelete(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> deleteMirror(
    String host,
  ) async {
    final request = RequestRepositoryDeleteMirror(
      repo: _handle,
      host: host,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  /// Export repository to file
  Future<String> export(
    String outputPath,
  ) async {
    final request = RequestRepositoryExport(
      repo: _handle,
      outputPath: outputPath,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponsePath(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<bool> fileExists(
    String path,
  ) async {
    final request = RequestRepositoryFileExists(
      repo: _handle,
      path: path,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<AccessMode> getAccessMode(
  ) async {
    final request = RequestRepositoryGetAccessMode(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseAccessMode(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<Duration?> getBlockExpiration(
  ) async {
    final request = RequestRepositoryGetBlockExpiration(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseDuration(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<List<int>> getCredentials(
  ) async {
    final request = RequestRepositoryGetCredentials(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseBytes(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  /// Returns the type of repository entry (file, directory, ...) or `None` if the entry doesn't
  /// exist.
  Future<EntryType?> getEntryType(
    String path,
  ) async {
    final request = RequestRepositoryGetEntryType(
      repo: _handle,
      path: path,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseEntryType(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<Duration?> getExpiration(
  ) async {
    final request = RequestRepositoryGetExpiration(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseDuration(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  /// Return the info-hash of the repository formatted as hex string. This can be used as a globally
  /// unique, non-secret identifier of the repository.
  Future<String> getInfoHash(
  ) async {
    final request = RequestRepositoryGetInfoHash(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseString(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<String?> getMetadata(
    String key,
  ) async {
    final request = RequestRepositoryGetMetadata(
      repo: _handle,
      key: key,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseString(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<String?> getMountPoint(
  ) async {
    final request = RequestRepositoryGetMountPoint(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponsePath(value: final value): return value;
      case ResponseNone(): return null;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<String> getPath(
  ) async {
    final request = RequestRepositoryGetPath(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponsePath(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<QuotaInfo> getQuota(
  ) async {
    final request = RequestRepositoryGetQuota(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseQuotaInfo(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<Stats> getStats(
  ) async {
    final request = RequestRepositoryGetStats(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseStats(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<Progress> getSyncProgress(
  ) async {
    final request = RequestRepositoryGetSyncProgress(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseProgress(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<bool> isDhtEnabled(
  ) async {
    final request = RequestRepositoryIsDhtEnabled(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<bool> isPexEnabled(
  ) async {
    final request = RequestRepositoryIsPexEnabled(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<bool> isSyncEnabled(
  ) async {
    final request = RequestRepositoryIsSyncEnabled(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<bool> mirrorExists(
    String host,
  ) async {
    final request = RequestRepositoryMirrorExists(
      repo: _handle,
      host: host,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<String> mount(
  ) async {
    final request = RequestRepositoryMount(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponsePath(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> move(
    String dst,
  ) async {
    final request = RequestRepositoryMove(
      repo: _handle,
      dst: dst,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> moveEntry(
    String src,
    String dst,
  ) async {
    final request = RequestRepositoryMoveEntry(
      repo: _handle,
      src: src,
      dst: dst,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<File> openFile(
    String path,
  ) async {
    final request = RequestRepositoryOpenFile(
      repo: _handle,
      path: path,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseFile(value: final value): return File(_client, value);
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<List<DirectoryEntry>> readDirectory(
    String path,
  ) async {
    final request = RequestRepositoryReadDirectory(
      repo: _handle,
      path: path,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseDirectoryEntries(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  /// Removes the directory at the given path from the repository. If `recursive` is true it removes
  /// also the contents, otherwise the directory must be empty.
  Future<void> removeDirectory(
    String path,
    bool recursive,
  ) async {
    final request = RequestRepositoryRemoveDirectory(
      repo: _handle,
      path: path,
      recursive: recursive,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  /// Remove (delete) the file at the given path from the repository.
  Future<void> removeFile(
    String path,
  ) async {
    final request = RequestRepositoryRemoveFile(
      repo: _handle,
      path: path,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> resetAccess(
    String token,
  ) async {
    final request = RequestRepositoryResetAccess(
      repo: _handle,
      token: token,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setAccess(
    AccessChange? read,
    AccessChange? write,
  ) async {
    final request = RequestRepositorySetAccess(
      repo: _handle,
      read: read,
      write: write,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setAccessMode(
    AccessMode accessMode,
    LocalSecret? localSecret,
  ) async {
    final request = RequestRepositorySetAccessMode(
      repo: _handle,
      accessMode: accessMode,
      localSecret: localSecret,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setBlockExpiration(
    Duration? value,
  ) async {
    final request = RequestRepositorySetBlockExpiration(
      repo: _handle,
      value: value,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setCredentials(
    List<int> credentials,
  ) async {
    final request = RequestRepositorySetCredentials(
      repo: _handle,
      credentials: credentials,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setDhtEnabled(
    bool enabled,
  ) async {
    final request = RequestRepositorySetDhtEnabled(
      repo: _handle,
      enabled: enabled,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setExpiration(
    Duration? value,
  ) async {
    final request = RequestRepositorySetExpiration(
      repo: _handle,
      value: value,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<bool> setMetadata(
    List<MetadataEdit> edits,
  ) async {
    final request = RequestRepositorySetMetadata(
      repo: _handle,
      edits: edits,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setPexEnabled(
    bool enabled,
  ) async {
    final request = RequestRepositorySetPexEnabled(
      repo: _handle,
      enabled: enabled,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setQuota(
    StorageSize? value,
  ) async {
    final request = RequestRepositorySetQuota(
      repo: _handle,
      value: value,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> setSyncEnabled(
    bool enabled,
  ) async {
    final request = RequestRepositorySetSyncEnabled(
      repo: _handle,
      enabled: enabled,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<String> share(
    LocalSecret? localSecret,
    AccessMode accessMode,
  ) async {
    final request = RequestRepositoryShare(
      repo: _handle,
      localSecret: localSecret,
      accessMode: accessMode,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseShareToken(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> subscribe(
  ) async {
    final request = RequestRepositorySubscribe(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> unmount(
  ) async {
    final request = RequestRepositoryUnmount(
      repo: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
}

class ShareToken {
  final Client _client;
  final String _value;
  
  ShareToken(this._client, this._value);
  
  Future<AccessMode> getAccessMode(
  ) async {
    final request = RequestShareTokenGetAccessMode(
      token: _value,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseAccessMode(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  /// Return the info-hash of the repository corresponding to the given token, formatted as hex
  /// string.
  ///
  /// See also: [repository_get_info_hash]
  Future<String> getInfoHash(
  ) async {
    final request = RequestShareTokenGetInfoHash(
      token: _value,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseString(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<String> getSuggestedName(
  ) async {
    final request = RequestShareTokenGetSuggestedName(
      token: _value,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseString(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<bool> mirrorExists(
    String host,
  ) async {
    final request = RequestShareTokenMirrorExists(
      token: _value,
      host: host,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseBool(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<String> normalize(
  ) async {
    final request = RequestShareTokenNormalize(
      token: _value,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseShareToken(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
}

class File {
  final Client _client;
  final FileHandle _handle;
  
  File(this._client, this._handle);
  
  Future<void> close(
  ) async {
    final request = RequestFileClose(
      file: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> flush(
  ) async {
    final request = RequestFileFlush(
      file: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<int> len(
  ) async {
    final request = RequestFileLen(
      file: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseU64(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  /// Returns sync progress of the given file.
  Future<int> progress(
  ) async {
    final request = RequestFileProgress(
      file: _handle,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseU64(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  /// Reads `len` bytes from the file starting at `offset` bytes from the beginning of the file.
  Future<List<int>> read(
    int offset,
    int len,
  ) async {
    final request = RequestFileRead(
      file: _handle,
      offset: offset,
      len: len,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseBytes(value: final value): return value;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> truncate(
    int len,
  ) async {
    final request = RequestFileTruncate(
      file: _handle,
      len: len,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
  Future<void> write(
    int offset,
    List<int> data,
  ) async {
    final request = RequestFileWrite(
      file: _handle,
      offset: offset,
      data: data,
    );
    final response = await _client.invoke(request);
    switch (response) {
      case ResponseNone(): return;
      default: throw InvalidData('unexpected response');
    }
  }
  
}

