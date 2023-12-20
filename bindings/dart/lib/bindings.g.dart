enum AccessMode {
  blind,
  read,
  write,
  ;

  static AccessMode decode(int n) {
    switch (n) {
      case 0: return AccessMode.blind;
      case 1: return AccessMode.read;
      case 2: return AccessMode.write;
      default: throw ArgumentError('invalid value: $n');
    }
  }

  int encode() {
    switch (this) {
      case AccessMode.blind: return 0;
      case AccessMode.read: return 1;
      case AccessMode.write: return 2;
    }
  }

}

enum EntryType {
  file,
  directory,
  ;

  static EntryType decode(int n) {
    switch (n) {
      case 1: return EntryType.file;
      case 2: return EntryType.directory;
      default: throw ArgumentError('invalid value: $n');
    }
  }

  int encode() {
    switch (this) {
      case EntryType.file: return 1;
      case EntryType.directory: return 2;
    }
  }

}

enum ErrorCode {
  ok,
  store,
  permissionDenied,
  malformedData,
  entryExists,
  entryNotFound,
  ambiguousEntry,
  directoryNotEmpty,
  operationNotSupported,
  config,
  invalidArgument,
  malformedMessage,
  storageVersionMismatch,
  connectionLost,
  vfsInvalidMountPoint,
  vfsDriverInstall,
  vfsBackend,
  other,
  ;

  static ErrorCode decode(int n) {
    switch (n) {
      case 0: return ErrorCode.ok;
      case 1: return ErrorCode.store;
      case 2: return ErrorCode.permissionDenied;
      case 3: return ErrorCode.malformedData;
      case 4: return ErrorCode.entryExists;
      case 5: return ErrorCode.entryNotFound;
      case 6: return ErrorCode.ambiguousEntry;
      case 7: return ErrorCode.directoryNotEmpty;
      case 8: return ErrorCode.operationNotSupported;
      case 10: return ErrorCode.config;
      case 11: return ErrorCode.invalidArgument;
      case 12: return ErrorCode.malformedMessage;
      case 13: return ErrorCode.storageVersionMismatch;
      case 14: return ErrorCode.connectionLost;
      case 2048: return ErrorCode.vfsInvalidMountPoint;
      case 2049: return ErrorCode.vfsDriverInstall;
      case 2050: return ErrorCode.vfsBackend;
      case 65535: return ErrorCode.other;
      default: throw ArgumentError('invalid value: $n');
    }
  }

  int encode() {
    switch (this) {
      case ErrorCode.ok: return 0;
      case ErrorCode.store: return 1;
      case ErrorCode.permissionDenied: return 2;
      case ErrorCode.malformedData: return 3;
      case ErrorCode.entryExists: return 4;
      case ErrorCode.entryNotFound: return 5;
      case ErrorCode.ambiguousEntry: return 6;
      case ErrorCode.directoryNotEmpty: return 7;
      case ErrorCode.operationNotSupported: return 8;
      case ErrorCode.config: return 10;
      case ErrorCode.invalidArgument: return 11;
      case ErrorCode.malformedMessage: return 12;
      case ErrorCode.storageVersionMismatch: return 13;
      case ErrorCode.connectionLost: return 14;
      case ErrorCode.vfsInvalidMountPoint: return 2048;
      case ErrorCode.vfsDriverInstall: return 2049;
      case ErrorCode.vfsBackend: return 2050;
      case ErrorCode.other: return 65535;
    }
  }

}

enum LogLevel {
  error,
  warn,
  info,
  debug,
  trace,
  ;

  static LogLevel decode(int n) {
    switch (n) {
      case 1: return LogLevel.error;
      case 2: return LogLevel.warn;
      case 3: return LogLevel.info;
      case 4: return LogLevel.debug;
      case 5: return LogLevel.trace;
      default: throw ArgumentError('invalid value: $n');
    }
  }

  int encode() {
    switch (this) {
      case LogLevel.error: return 1;
      case LogLevel.warn: return 2;
      case LogLevel.info: return 3;
      case LogLevel.debug: return 4;
      case LogLevel.trace: return 5;
    }
  }

}

enum NetworkEvent {
  protocolVersionMismatch,
  peerSetChange,
  ;

  static NetworkEvent decode(int n) {
    switch (n) {
      case 0: return NetworkEvent.protocolVersionMismatch;
      case 1: return NetworkEvent.peerSetChange;
      default: throw ArgumentError('invalid value: $n');
    }
  }

  int encode() {
    switch (this) {
      case NetworkEvent.protocolVersionMismatch: return 0;
      case NetworkEvent.peerSetChange: return 1;
    }
  }

}

enum PeerSource {
  userProvided,
  listener,
  localDiscovery,
  dht,
  peerExchange,
  ;

  static PeerSource decode(int n) {
    switch (n) {
      case 0: return PeerSource.userProvided;
      case 1: return PeerSource.listener;
      case 2: return PeerSource.localDiscovery;
      case 3: return PeerSource.dht;
      case 4: return PeerSource.peerExchange;
      default: throw ArgumentError('invalid value: $n');
    }
  }

  int encode() {
    switch (this) {
      case PeerSource.userProvided: return 0;
      case PeerSource.listener: return 1;
      case PeerSource.localDiscovery: return 2;
      case PeerSource.dht: return 3;
      case PeerSource.peerExchange: return 4;
    }
  }

}

enum PeerStateKind {
  known,
  connecting,
  handshaking,
  active,
  ;

  static PeerStateKind decode(int n) {
    switch (n) {
      case 0: return PeerStateKind.known;
      case 1: return PeerStateKind.connecting;
      case 2: return PeerStateKind.handshaking;
      case 3: return PeerStateKind.active;
      default: throw ArgumentError('invalid value: $n');
    }
  }

  int encode() {
    switch (this) {
      case PeerStateKind.known: return 0;
      case PeerStateKind.connecting: return 1;
      case PeerStateKind.handshaking: return 2;
      case PeerStateKind.active: return 3;
    }
  }

}

