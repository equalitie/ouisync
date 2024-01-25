import 'dart:async';
import 'dart:collection';
import 'dart:ffi';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:ffi/ffi.dart';
import 'package:hex/hex.dart';

import 'bindings.dart';
import 'client.dart';
import 'native_channels.dart';
import 'state_monitor.dart';

export 'bindings.dart'
    show
        AccessMode,
        EntryType,
        ErrorCode,
        LogLevel,
        NetworkEvent,
        PeerSource,
        PeerStateKind,
        SessionKind;
export 'native_channels.dart' show NativeChannels;

const bool debugTrace = false;

/// Entry point to the ouisync bindings. A session should be opened at the start of the application
/// and closed at the end. There can be only one session at the time.
class Session {
  int _handle;
  final Client client;
  final Subscription _networkSubscription;
  String? _mountPoint;

  int get handle => _handle;

  Session._(this._handle, this.client)
      : _networkSubscription = Subscription(client, "network", null) {
    NativeChannels.session = this;
  }

  /// Creates a new session in this process.
  /// [configPath] is a path to a directory where configuration files shall be stored. If it
  /// doesn't exists, it will be created.
  /// [logPath] is a path to the log file. If null, logs will be printed to standard output.
  static Session create({
    SessionKind kind = SessionKind.shared,
    required String configPath,
    String? logPath,
  }) {
    if (debugTrace) {
      print("Session.open $configPath");
    }

    final recvPort = ReceivePort();
    final result = _withPoolSync((pool) => bindings.session_create(
          kind.encode(),
          pool.toNativeUtf8(configPath),
          logPath != null ? pool.toNativeUtf8(logPath) : nullptr,
          NativeApi.postCObject,
          recvPort.sendPort.nativePort,
        ));

    final errorCode = ErrorCode.decode(result.error_code);

    int handle;

    if (errorCode == ErrorCode.ok) {
      handle = result.session;
    } else {
      final errorMessage = result.error_message.cast<Utf8>().intoDartString();
      throw Error(errorCode, errorMessage);
    }

    final client = Client(handle, recvPort);

    return Session._(handle, client);
  }

  String? get mountPoint => _mountPoint;

  // Mount all repositories that are open now or in future in read or
  // read/write mode into the `mountPoint`. The `mountPoint` may point to an
  // empty directory or may be a drive letter.
  Future<void> mountAllRepositories(String mountPoint) async {
    await client.invoke<void>("repository_mount_all", mountPoint);
    _mountPoint = mountPoint;
  }

  /// Initialize network from config. Fall back to the provided defaults if the corresponding
  /// config entries don't exist.
  Future<void> initNetwork({
    bool defaultPortForwardingEnabled = false,
    bool defaultLocalDiscoveryEnabled = false,
  }) =>
      client.invoke<void>("network_init", {
        'port_forwarding_enabled': defaultPortForwardingEnabled,
        'local_discovery_enabled': defaultLocalDiscoveryEnabled,
      });

  /// Binds network to the specified addresses.
  Future<void> bindNetwork({
    String? quicV4,
    String? quicV6,
    String? tcpV4,
    String? tcpV6,
  }) async {
    await client.invoke<void>("network_bind", {
      'quic_v4': quicV4,
      'quic_v6': quicV6,
      'tcp_v4': tcpV4,
      'tcp_v6': tcpV6,
    });
  }

  Stream<NetworkEvent> get networkEvents =>
      _networkSubscription.stream.map((raw) => NetworkEvent.decode(raw as int));

  Future<void> addUserProvidedPeer(String addr) =>
      client.invoke<void>('network_add_user_provided_peer', addr);

  Future<void> removeUserProvidedPeer(String addr) =>
      client.invoke<void>('network_remove_user_provided_peer', addr);

  Future<List<String>> get userProvidedPeers => client
      .invoke<List<Object?>>('network_user_provided_peers')
      .then((list) => list.cast<String>());

  Future<String?> get tcpListenerLocalAddressV4 =>
      client.invoke<String?>('network_tcp_listener_local_addr_v4');

  Future<String?> get tcpListenerLocalAddressV6 =>
      client.invoke<String?>('network_tcp_listener_local_addr_v6');

  Future<String?> get quicListenerLocalAddressV4 =>
      client.invoke<String?>('network_quic_listener_local_addr_v4');

  Future<String?> get quicListenerLocalAddressV6 =>
      client.invoke<String?>('network_quic_listener_local_addr_v6');

  Future<String?> get externalAddressV4 =>
      client.invoke<String?>('network_external_addr_v4');

  Future<String?> get externalAddressV6 =>
      client.invoke<String?>('network_external_addr_v6');

  Future<String?> get natBehavior =>
      client.invoke<String?>('network_nat_behavior');

  Future<TrafficStats> get trafficStats => client
      .invoke<List<Object?>>('network_traffic_stats')
      .then((list) => TrafficStats.decode(list));

  /// Gets a stream that yields lists of known peers.
  Stream<List<PeerInfo>> get onPeersChange async* {
    await for (final _ in networkEvents) {
      yield await peers;
    }
  }

  Future<List<PeerInfo>> get peers => client
      .invoke<List<Object?>>('network_known_peers')
      .then(PeerInfo.decodeAll);

  StateMonitor get rootStateMonitor => StateMonitor.getRoot(this);

  Future<int> get currentProtocolVersion =>
      client.invoke<int>('network_current_protocol_version');

  Future<int> get highestSeenProtocolVersion =>
      client.invoke<int>('network_highest_seen_protocol_version');

  /// Is port forwarding (UPnP) enabled?
  Future<bool> get isPortForwardingEnabled =>
      client.invoke<bool>('network_is_port_forwarding_enabled');

  /// Enable/disable port forwarding (UPnP)
  Future<void> setPortForwardingEnabled(bool enabled) =>
      client.invoke<void>('network_set_port_forwarding_enabled', enabled);

  /// Is local discovery enabled?
  Future<bool> get isLocalDiscoveryEnabled =>
      client.invoke<bool>('network_is_local_discovery_enabled');

  /// Enable/disable local discovery
  Future<void> setLocalDiscoveryEnabled(bool enabled) =>
      client.invoke<void>('network_set_local_discovery_enabled', enabled);

  Future<String> get thisRuntimeId =>
      client.invoke<String>('network_this_runtime_id');

  Future<void> addStorageServer(String host) =>
      client.invoke<void>('network_add_storage_server', host);

  /// Try to gracefully close connections to peers then close the session.
  ///
  /// Note that this function is idempotent with itself as well as with the
  /// `closeSync` function.
  Future<void> close() async {
    if (_handle == 0) {
      return;
    }

    final h = _handle;
    _handle = 0;

    await _networkSubscription.close();

    NativeChannels.session = null;

    await _shutdownNetwork();
    bindings.session_close(h);
  }

  /// Try to gracefully close connections to peers then close the session.
  /// Async functions don't work reliably when the dart engine is being
  /// shutdown (on app exit). In those situations the network needs to be shut
  /// down using a blocking call.
  ///
  /// Note that this function is idempotent with itself as well as with the
  /// `close` function.
  void closeSync() {
    if (_handle == 0) {
      return;
    }

    final h = _handle;
    _handle = 0;

    unawaited(_networkSubscription.close());

    NativeChannels.session = null;
    bindings.session_shutdown_network_and_close(h);
  }

  /// Try to gracefully close connections to peers.
  Future<void> _shutdownNetwork() async {
    await client.invoke<void>('network_shutdown');
  }
}

class PeerInfo {
  final String addr;
  final PeerSource source;
  final PeerStateKind state;
  final String? runtimeId;

  PeerInfo({
    required this.addr,
    required this.source,
    required this.state,
    this.runtimeId,
  });

  static PeerInfo decode(Object? raw) {
    final list = raw as List<Object?>;

    final addr = list[0] as String;
    final source = PeerSource.decode(list[1] as int);
    final rawState = list[2];

    PeerStateKind state;
    String? runtimeId;

    if (rawState is int) {
      state = PeerStateKind.decode(rawState);
    } else if (rawState is List) {
      state = PeerStateKind.decode(rawState[0] as int);
      runtimeId = HEX.encode(rawState[1] as Uint8List);
    } else {
      throw Exception('invalid peer info state');
    }

    return PeerInfo(
      addr: addr,
      source: source,
      state: state,
      runtimeId: runtimeId,
    );
  }

  static List<PeerInfo> decodeAll(List<Object?> raw) =>
      raw.map((rawItem) => PeerInfo.decode(rawItem)).toList();

  @override
  String toString() =>
      '$runtimeType(addr: $addr, source: $source, state: $state, runtimeId: $runtimeId)';
}

class TrafficStats {
  final int send;
  final int recv;

  const TrafficStats({required this.send, required this.recv});

  static TrafficStats decode(List<Object?> raw) {
    final send = raw[0] as int;
    final recv = raw[1] as int;

    return TrafficStats(send: send, recv: recv);
  }

  @override
  String toString() => '$runtimeType(send: $send, recv: $recv)';
}

/// A reference to a ouisync repository.
class Repository {
  final Session session;
  final int handle;
  final String _store;
  final Subscription _subscription;

  Repository._(this.session, this.handle, this._store)
      : _subscription = Subscription(session.client, "repository", handle);

  /// Creates a new repository and set access to it based on the following table:
  ///
  /// local_read_password  |  local_write_password  |  token access  |  result
  /// ---------------------+------------------------+----------------+------------------------------
  /// null or any          |  null or any           |  blind         |  blind replica
  /// null                 |  null or any           |  read          |  read without password
  /// read_pwd             |  null or any           |  read          |  read with read_pwd as password
  /// null                 |  null                  |  write         |  read and write without password
  /// any                  |  null                  |  write         |  read (only!) with password
  /// null                 |  any                   |  write         |  read without password, require password for writing
  /// any                  |  any                   |  write         |  read with one password, write with (possibly same) one
  static Future<Repository> create(
    Session session, {
    required String store,
    required String? readPassword,
    required String? writePassword,
    ShareToken? shareToken,
  }) async {
    if (debugTrace) {
      print("Repository.create $store");
    }

    final handle = await session.client.invoke<int>(
      'repository_create',
      {
        'path': store,
        'read_password': readPassword,
        'write_password': writePassword,
        'share_token': shareToken?.token
      },
    );

    return Repository._(session, handle, store);
  }

  /// Opens an existing repository.
  static Future<Repository> open(
    Session session, {
    required String store,
    String? password,
  }) async {
    if (debugTrace) {
      print("Repository.open $store");
    }

    final handle = await session.client.invoke<int>('repository_open', {
      'path': store,
      'password': password,
    });

    return Repository._(session, handle, store);
  }

  /// Opens an existing repository using a reopen token.
  static Future<Repository> reopen(
    Session session, {
    required String store,
    required Uint8List token,
  }) async {
    final handle = await session.client.invoke<int>('repository_reopen', {
      'path': store,
      'token': token,
    });

    return Repository._(session, handle, store);
  }

  /// Close the repository. Accessing the repository after it's been closed is an error.
  Future<void> close() async {
    if (debugTrace) {
      print("Repository.close");
    }

    await _subscription.close();
    await session.client.invoke('repository_close', handle);
  }

  /// Creates a reopen token to be used to reopen this repository in the same access mode as it has
  /// now.
  Future<Uint8List> createReopenToken() => session.client
      .invoke<Uint8List>('repository_create_reopen_token', handle);

  /// Returns the type (file, directory, ..) of the entry at [path]. Returns `null` if the entry
  /// doesn't exists.
  Future<EntryType?> type(String path) async {
    if (debugTrace) {
      print("Repository.type $path");
    }

    final raw = await session.client.invoke<int?>('repository_entry_type', {
      'repository': handle,
      'path': path,
    });

    return raw != null ? EntryType.decode(raw) : null;
  }

  /// Returns whether the entry (file or directory) at [path] exists.
  Future<bool> exists(String path) async {
    if (debugTrace) {
      print("Repository.exists $path");
    }

    return await type(path) != null;
  }

  /// Move/rename the file/directory from [src] to [dst].
  Future<void> move(String src, String dst) async {
    if (debugTrace) {
      print("Repository.move $src -> $dst");
    }

    await session.client.invoke<void>('repository_move_entry', {
      'repository': handle,
      'src': src,
      'dst': dst,
    });
  }

  Stream<void> get events => _subscription.stream.cast<void>();

  Future<bool> get isDhtEnabled async {
    if (debugTrace) {
      print("Repository.isDhtEnabled");
    }

    return await session.client
        .invoke<bool>('repository_is_dht_enabled', handle);
  }

  Future<void> setDhtEnabled(bool enabled) async {
    if (debugTrace) {
      print("Repository.setDhtEnabled($enabled)");
    }

    await session.client.invoke<void>('repository_set_dht_enabled', {
      'repository': handle,
      'enabled': enabled,
    });
  }

  Future<bool> get isPexEnabled =>
      session.client.invoke<bool>('repository_is_pex_enabled', handle);

  Future<void> setPexEnabled(bool enabled) =>
      session.client.invoke<void>('repository_set_pex_enabled', {
        'repository': handle,
        'enabled': enabled,
      });

  Future<AccessMode> get accessMode {
    if (debugTrace) {
      print("Repository.get accessMode");
    }

    return session.client
        .invoke<int>('repository_access_mode', handle)
        .then((n) => AccessMode.decode(n));
  }

  /// Create a share token providing access to this repository with the given mode. Can optionally
  /// specify repository name which will be included in the token and suggested to the recipient.
  Future<ShareToken> createShareToken({
    required AccessMode accessMode,
    String? password,
    String? name,
  }) {
    if (debugTrace) {
      print("Repository.createShareToken");
    }

    return session.client.invoke<String>('repository_create_share_token', {
      'repository': handle,
      'password': password,
      'access_mode': accessMode.encode(),
      'name': name,
    }).then((token) => ShareToken._(session, token));
  }

  Future<Progress> get syncProgress => session.client
      .invoke<List<Object?>>('repository_sync_progress', handle)
      .then(Progress.decode);

  StateMonitor get stateMonitor => StateMonitor.getRoot(session)
      .child(MonitorId.expectUnique("Repositories"))
      .child(MonitorId.expectUnique(_store));

  Future<String> get infoHash =>
      session.client.invoke<String>("repository_info_hash", handle);

  Future<void> setReadWriteAccess({
    required String? oldPassword,
    required String newPassword,
    required ShareToken? shareToken,
  }) =>
      session.client.invoke<void>('repository_set_read_and_write_access', {
        'repository': handle,
        'old_password': oldPassword,
        'new_password': newPassword,
        'share_token': shareToken?.toString(),
      });

  Future<void> setReadAccess({
    required String newPassword,
    required ShareToken? shareToken,
  }) =>
      session.client.invoke<void>('repository_set_read_access', {
        'repository': handle,
        'password': newPassword,
        'share_token': shareToken?.toString(),
      });

  Future<String> hexDatabaseId() async {
    final bytes = await session.client
        .invoke<Uint8List>("repository_database_id", handle);
    return HEX.encode(bytes);
  }

  /// Create mirror of this repository on the storage servers.
  Future<void> mirror() => session.client.invoke<void>('repository_mirror', {
        'repository': handle,
      });
}

class ShareToken {
  final Session session;
  final String token;

  ShareToken._(this.session, this.token);

  static Future<ShareToken> fromString(Session session, String s) =>
      session.client
          .invoke<String>('share_token_normalize', s)
          .then((s) => ShareToken._(session, s));

  /// Get the suggested repository name from the share token.
  Future<String> get suggestedName =>
      session.client.invoke<String>('share_token_suggested_name', token);

  Future<String> get infoHash =>
      session.client.invoke<String>('share_token_info_hash', token);

  /// Get the access mode the share token provides.
  Future<AccessMode> get mode => session.client
      .invoke<int>('share_token_mode', token)
      .then((n) => AccessMode.decode(n));

  @override
  String toString() => token;

  @override
  bool operator ==(Object other) => other is ShareToken && other.token == token;

  @override
  int get hashCode => token.hashCode;
}

class Progress {
  final int value;
  final int total;

  Progress(this.value, this.total);

  static Progress decode(List<Object?> raw) {
    final value = raw[0] as int;
    final total = raw[1] as int;

    return Progress(value, total);
  }

  @override
  String toString() => '$value/$total';

  @override
  bool operator ==(Object other) =>
      other is Progress && other.value == value && other.total == total;

  @override
  int get hashCode => Object.hash(value, total);
}

/// Single entry of a directory.
class DirEntry {
  final String name;
  final EntryType entryType;

  DirEntry(this.name, this.entryType);

  static DirEntry decode(Object? raw) {
    final map = raw as List<Object?>;
    final name = map[0] as String;
    final type = map[1] as int;

    return DirEntry(name, EntryType.decode(type));
  }
}

/// A reference to a directory (folder) in a [Repository].
///
/// This class is [Iterable], yielding the directory entries.
///
/// Note: Currently this is a read-only snapshot of the directory at the time is was opened.
/// Subsequent external changes to the directory (e.g. added files) are not recognized and the
/// directory needs to be manually reopened to do so.
class Directory with IterableMixin<DirEntry> {
  final List<DirEntry> entries;

  Directory._(this.entries);

  /// Opens a directory of [repo] at [path].
  ///
  /// Throws if [path] doesn't exist or is not a directory.
  ///
  /// Note: don't forget to [close] it when no longer needed.
  static Future<Directory> open(Repository repo, String path) async {
    if (debugTrace) {
      print("Directory.open $path");
    }

    final rawEntries = await repo.session.client.invoke<List<Object?>>(
      'directory_open',
      {
        'repository': repo.handle,
        'path': path,
      },
    );
    final entries = rawEntries.map(DirEntry.decode).toList();

    return Directory._(entries);
  }

  /// Creates a new directory in [repo] at [path].
  ///
  /// Throws if [path] already exists of if the parent of [path] doesn't exists.
  static Future<void> create(Repository repo, String path) {
    if (debugTrace) {
      print("Directory.create $path");
    }

    return repo.session.client.invoke<void>('directory_create', {
      'repository': repo.handle,
      'path': path,
    });
  }

  /// Remove a directory from [repo] at [path]. If [recursive] is false (which is the default),
  /// the directory must be empty otherwise an exception is thrown. If [recursive] it is true, the
  /// content of the directory is removed as well.
  static Future<void> remove(
    Repository repo,
    String path, {
    bool recursive = false,
  }) {
    if (debugTrace) {
      print("Directory.remove $path");
    }

    return repo.session.client.invoke<void>('directory_remove', {
      'repository': repo.handle,
      'path': path,
      'recursive': recursive,
    });
  }

  /// Returns an [Iterator] to iterate over entries of this directory.
  @override
  Iterator<DirEntry> get iterator => entries.iterator;
}

/// Reference to a file in a [Repository].
class File {
  Session session;
  final int handle;

  File._(this.session, this.handle);

  static const defaultChunkSize = 1024;

  /// Opens an existing file from [repo] at [path].
  ///
  /// Throws if [path] doesn't exists or is a directory.
  static Future<File> open(Repository repo, String path) async {
    if (debugTrace) {
      print("File.open");
    }

    return File._(
        repo.session,
        await repo.session.client.invoke<int>('file_open', {
          'repository': repo.handle,
          'path': path,
        }));
  }

  /// Creates a new file in [repo] at [path].
  ///
  /// Throws if [path] already exists of if the parent of [path] doesn't exists.
  static Future<File> create(Repository repo, String path) async {
    if (debugTrace) {
      print("File.create $path");
    }

    return File._(
        repo.session,
        await repo.session.client.invoke<int>('file_create', {
          'repository': repo.handle,
          'path': path,
        }));
  }

  /// Removes (deletes) a file at [path] from [repo].
  static Future<void> remove(Repository repo, String path) {
    if (debugTrace) {
      print("File.remove $path");
    }

    return repo.session.client.invoke<void>('file_remove', {
      'repository': repo.handle,
      'path': path,
    });
  }

  /// Flushed and closes this file.
  Future<void> close() {
    if (debugTrace) {
      print("File.close");
    }

    return session.client.invoke<void>('file_close', handle);
  }

  /// Flushes any pending writes to this file.
  Future<void> flush() {
    if (debugTrace) {
      print("File.flush");
    }

    return session.client.invoke<void>('file_flush', handle);
  }

  /// Read [size] bytes from this file, starting at [offset].
  ///
  /// To read the whole file at once:
  ///
  /// ```dart
  /// final length = await file.length;
  /// final content = await file.read(0, length);
  /// ```
  ///
  /// To read the whole file in chunks:
  ///
  /// ```dart
  /// final chunkSize = 1024;
  /// var offset = 0;
  ///
  /// while (true) {
  ///   final chunk = await file.read(offset, chunkSize);
  ///   offset += chunk.length;
  ///
  ///   doSomethingWithTheChunk(chunk);
  ///
  ///   if (chunk.length < chunkSize) {
  ///     break;
  ///   }
  /// }
  /// ```
  Future<List<int>> read(int offset, int size) {
    if (debugTrace) {
      print("File.read");
    }

    return session.client.invoke<Uint8List>(
        'file_read', {'file': handle, 'offset': offset, 'len': size});
  }

  /// Write [data] to this file starting at [offset].
  Future<void> write(int offset, List<int> data) {
    if (debugTrace) {
      print("File.write");
    }

    return session.client.invoke<void>('file_write', {
      'file': handle,
      'offset': offset,
      'data': Uint8List.fromList(data),
    });
  }

  /// Truncate the file to [size] bytes.
  Future<void> truncate(int size) {
    if (debugTrace) {
      print("File.truncate");
    }

    return session.client.invoke<void>('file_truncate', {
      'file': handle,
      'len': size,
    });
  }

  /// Returns the length of this file in bytes.
  Future<int> get length {
    if (debugTrace) {
      print("File.length");
    }

    return session.client.invoke<int>('file_len', handle);
  }

  Future<int> get progress =>
      session.client.invoke<int>('file_progress', handle);

  /// Copy the contents of the file into the provided raw file descriptor.
  Future<void> copyToRawFd(int fd) {
    if (debugTrace) {
      print("File.copyToRawFd");
    }

    return _invoke(
      (port) => bindings.file_copy_to_raw_fd(
        session.handle,
        handle,
        fd,
        NativeApi.postCObject,
        port,
      ),
    );
  }
}

/// Print log message
void logPrint(LogLevel level, String scope, String message) =>
    _withPoolSync((pool) => bindings.log_print(
          level.encode(),
          pool.toNativeUtf8(scope),
          pool.toNativeUtf8(message),
        ));

/// The exception type throws from this library.
class Error implements Exception {
  final String message;
  final ErrorCode code;

  Error(this.code, this.message);

  @override
  String toString() => message;
}

// Private helpers to simplify working with the native API:

// Call the sync function passing it a [_Pool] which will be released when the function returns.
T _withPoolSync<T>(T Function(_Pool) fun) {
  final pool = _Pool();

  try {
    return fun(pool);
  } finally {
    pool.release();
  }
}

// Helper to invoke a native async function.
Future<T> _invoke<T>(void Function(int) fun) async {
  final recvPort = ReceivePort();

  try {
    fun(recvPort.sendPort.nativePort);

    ErrorCode? code;

    // Is there a better way to retrieve the first two values of a stream?
    await for (var item in recvPort) {
      if (code == null) {
        code = ErrorCode.decode(item as int);
      } else if (code == ErrorCode.ok) {
        return item as T;
      } else {
        throw Error(code, item as String);
      }
    }

    throw Exception('invoked native async function did not produce any result');
  } finally {
    recvPort.close();
  }
}

// Allocator that tracks all allocations and frees them all at the same time.
class _Pool implements Allocator {
  List<Pointer<NativeType>> ptrs = [];

  @override
  Pointer<T> allocate<T extends NativeType>(int byteCount, {int? alignment}) {
    final ptr = malloc.allocate<T>(byteCount, alignment: alignment);
    ptrs.add(ptr);
    return ptr;
  }

  @override
  void free(Pointer<NativeType> ptr) {
    // free on [release]
  }

  void release() {
    for (var ptr in ptrs) {
      malloc.free(ptr);
    }
  }

  // Convenience function to convert a dart string to a C-style nul-terminated utf-8 encoded
  // string pointer. The pointer is allocated using this pool.
  Pointer<Char> toNativeUtf8(String str) =>
      str.toNativeUtf8(allocator: this).cast<Char>();
}

extension Utf8Pointer on Pointer<Utf8> {
  // Similar to [toDartString] but also deallocates the original pointer.
  String intoDartString() {
    final string = toDartString();
    freeString(this);
    return string;
  }
}

// Free a pointer that was allocated by the native side.
void freeString(Pointer<Utf8> ptr) {
  bindings.free_string(ptr.cast<Char>());
}
