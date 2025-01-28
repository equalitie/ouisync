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

enum ErrorCode {
  ok,
  permissionDenied,
  invalidInput,
  invalidData,
  alreadyExists,
  notFound,
  ambiguous,
  unsupported,
  connectionRefused,
  connectionAborted,
  transportError,
  listenerBind,
  listenerAccept,
  storeError,
  isDirectory,
  notDirectory,
  directoryNotEmpty,
  resourceBusy,
  initializeRuntime,
  initializeLogger,
  config,
  tlsCertificatesNotFound,
  tlsCertificatesInvalid,
  tlsKeysNotFound,
  tlsConfig,
  vfsDriverInstallError,
  vfsOtherError,
  serviceAlreadyRunning,
  storeDirUnspecified,
  other,
  ;

  static ErrorCode decode(int n) {
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
      case 1028: return ErrorCode.listenerBind;
      case 1029: return ErrorCode.listenerAccept;
      case 2049: return ErrorCode.storeError;
      case 2050: return ErrorCode.isDirectory;
      case 2051: return ErrorCode.notDirectory;
      case 2052: return ErrorCode.directoryNotEmpty;
      case 2053: return ErrorCode.resourceBusy;
      case 4097: return ErrorCode.initializeRuntime;
      case 4098: return ErrorCode.initializeLogger;
      case 4099: return ErrorCode.config;
      case 4100: return ErrorCode.tlsCertificatesNotFound;
      case 4101: return ErrorCode.tlsCertificatesInvalid;
      case 4102: return ErrorCode.tlsKeysNotFound;
      case 4103: return ErrorCode.tlsConfig;
      case 4104: return ErrorCode.vfsDriverInstallError;
      case 4105: return ErrorCode.vfsOtherError;
      case 4106: return ErrorCode.serviceAlreadyRunning;
      case 4107: return ErrorCode.storeDirUnspecified;
      case 65535: return ErrorCode.other;
      default: throw ArgumentError('invalid value: $n');
    }
  }

  int encode() {
    switch (this) {
      case ErrorCode.ok: return 0;
      case ErrorCode.permissionDenied: return 1;
      case ErrorCode.invalidInput: return 2;
      case ErrorCode.invalidData: return 3;
      case ErrorCode.alreadyExists: return 4;
      case ErrorCode.notFound: return 5;
      case ErrorCode.ambiguous: return 6;
      case ErrorCode.unsupported: return 8;
      case ErrorCode.connectionRefused: return 1025;
      case ErrorCode.connectionAborted: return 1026;
      case ErrorCode.transportError: return 1027;
      case ErrorCode.listenerBind: return 1028;
      case ErrorCode.listenerAccept: return 1029;
      case ErrorCode.storeError: return 2049;
      case ErrorCode.isDirectory: return 2050;
      case ErrorCode.notDirectory: return 2051;
      case ErrorCode.directoryNotEmpty: return 2052;
      case ErrorCode.resourceBusy: return 2053;
      case ErrorCode.initializeRuntime: return 4097;
      case ErrorCode.initializeLogger: return 4098;
      case ErrorCode.config: return 4099;
      case ErrorCode.tlsCertificatesNotFound: return 4100;
      case ErrorCode.tlsCertificatesInvalid: return 4101;
      case ErrorCode.tlsKeysNotFound: return 4102;
      case ErrorCode.tlsConfig: return 4103;
      case ErrorCode.vfsDriverInstallError: return 4104;
      case ErrorCode.vfsOtherError: return 4105;
      case ErrorCode.serviceAlreadyRunning: return 4106;
      case ErrorCode.storeDirUnspecified: return 4107;
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

