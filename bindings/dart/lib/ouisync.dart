import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:io' as io;
import 'dart:typed_data';
import 'dart:math';

import 'package:hex/hex.dart';
import 'package:ouisync/exception.dart';

import 'bindings.dart';
import 'client.dart';
import 'server.dart';
import 'state_monitor.dart';

export 'bindings.dart'
    show
        AccessMode,
        EntryType,
        ErrorCode,
        LogLevel,
        NetworkEvent,
        PeerSource,
        PeerStateKind;
export 'exception.dart';

part 'local_secret.dart';

const bool debugTrace = false;

// Default log tag.
// HACK: if the tag doesn't start with 'flutter' then the logs won't show up in
// the app if built in release mode.
const defaultLogTag = 'flutter-ouisync';

/// Entry point to the ouisync bindings. A session should be opened at the start of the application
/// and closed at the end. There can be only one session at the time.
class Session {
  final Client _client;
  final Server? _server;

  Session._(this._client, this._server);

  /// Creates a new session in this process.
  /// [configPath] is a path to a directory where configuration files shall be stored. If it
  /// doesn't exists, it will be created.
  static Future<Session> create({
    required String socketPath,
    required String configPath,
    String? debugLabel,
  }) async {
    Server? server;

    // Try to start our own server but if one is already running connect to that one instead.
    try {
      server = await Server.start(
        socketPath: socketPath,
        configPath: configPath,
        debugLabel: debugLabel,
      );
    } on ServiceAlreadyRunning catch (_) {
      server = null;
    }

    final client = await SocketClient.connect(socketPath);

    return Session._(client, server);
  }

  /* TODO: do we still need this?

  // Creates a new session which forwards calls to Ouisync backend running in the
  // native code.
  // [channelName] is the name of the MethodChannel to be used, equally named channel
  // must be created and set up to listen to the commands in the native code.
  static Future<Session> createChanneled(String channelName,
      [void Function()? onClose]) async {
    final client = ChannelClient(channelName, onClose);
    await client.initialized;
    return Session._(client);
  }
  */

  Future<String?> get storeDir => _client.invoke('repository_get_store_dir');

  Future<void> setStoreDir(String path) =>
      _client.invoke('repository_set_store_dir', path);

  Future<String?> get mountRoot => _client.invoke('repository_get_mount_root');

  Future<void> setMountRoot(String? path) => _client.invoke(
        'repository_set_mount_root',
        path,
      );

  /// Initialize network from config. Fall back to the provided defaults if the corresponding
  /// config entries don't exist.
  Future<void> initNetwork({
    List<String> defaultBindAddrs = const [],
    bool defaultPortForwardingEnabled = false,
    bool defaultLocalDiscoveryEnabled = false,
  }) =>
      _client.invoke<void>("network_init", {
        'bind': defaultBindAddrs,
        'port_forwarding_enabled': defaultPortForwardingEnabled,
        'local_discovery_enabled': defaultLocalDiscoveryEnabled,
      });

  /// Binds network to the specified addresses.
  Future<void> bindNetwork(List<String> addrs) =>
      _client.invoke<void>("network_bind", addrs);

  Stream<NetworkEvent> get networkEvents => _client
      .subscribe('network', null)
      .map((raw) => NetworkEvent.decode(raw as int));

  Future<void> addUserProvidedPeers(List<String> addrs) =>
      _client.invoke<void>('network_add_user_provided_peers', addrs);

  Future<void> removeUserProvidedPeers(List<String> addrs) =>
      _client.invoke<void>('network_remove_user_provided_peers', addrs);

  Future<List<String>> get userProvidedPeers => _client
      .invoke<List<Object?>>('network_get_user_provided_peers')
      .then((list) => list.cast<String>());

  Future<List<String>> get listenerAddrs => _client
      .invoke<List<Object?>>('network_get_listener_addrs')
      .then((list) => list.cast<String>());

  Future<String?> get externalAddressV4 =>
      _client.invoke<String?>('network_external_addr_v4');

  Future<String?> get externalAddressV6 =>
      _client.invoke<String?>('network_external_addr_v6');

  Future<String?> get natBehavior =>
      _client.invoke<String?>('network_nat_behavior');

  Future<NetworkStats> get networkStats => _client
      .invoke<List<Object?>>('network_stats')
      .then((list) => NetworkStats.decode(list));

  Future<List<PeerInfo>> get peers => _client
      .invoke<List<Object?>>('network_get_peers')
      .then(PeerInfo.decodeAll);

  StateMonitor get rootStateMonitor => StateMonitor.getRoot(_client);

  Future<int> get currentProtocolVersion =>
      _client.invoke<int>('network_current_protocol_version');

  Future<int> get highestSeenProtocolVersion =>
      _client.invoke<int>('network_highest_seen_protocol_version');

  /// Is port forwarding (UPnP) enabled?
  Future<bool> get isPortForwardingEnabled =>
      _client.invoke<bool>('network_is_port_forwarding_enabled');

  /// Enable/disable port forwarding (UPnP)
  Future<void> setPortForwardingEnabled(bool enabled) =>
      _client.invoke<void>('network_set_port_forwarding_enabled', enabled);

  /// Is local discovery enabled?
  Future<bool> get isLocalDiscoveryEnabled =>
      _client.invoke<bool>('network_is_local_discovery_enabled');

  /// Enable/disable local discovery
  Future<void> setLocalDiscoveryEnabled(bool enabled) =>
      _client.invoke<void>('network_set_local_discovery_enabled', enabled);

  Future<String> get thisRuntimeId =>
      _client.invoke<String>('network_this_runtime_id');

  // Utility functions to generate password salts and to derive LocalSecretKey from LocalPasswords.

  Future<PasswordSalt> generatePasswordSalt() => _client
      .invoke<Uint8List>('password_generate_salt')
      .then((bytes) => PasswordSalt(bytes));

  Future<LocalSecretKey> deriveSecretKey(
    LocalPassword pwd,
    PasswordSalt salt,
  ) =>
      _client.invoke<Uint8List>('password_derive_secret_key', {
        'password': pwd.string,
        'salt': salt._bytes
      }).then((bytes) => LocalSecretKey(bytes));

  /// Try to gracefully close connections to peers then close the session.
  Future<void> close() async {
    await _client.close();
    await _server?.stop();
  }
}

class PeerInfo {
  final String addr;
  final PeerSource source;
  final PeerStateKind state;
  final String? runtimeId;
  final NetworkStats stats;

  PeerInfo({
    required this.addr,
    required this.source,
    required this.state,
    this.runtimeId,
    this.stats = const NetworkStats(),
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

    final stats = NetworkStats.decode(list[3] as List<Object?>);

    return PeerInfo(
      addr: addr,
      source: source,
      state: state,
      runtimeId: runtimeId,
      stats: stats,
    );
  }

  static List<PeerInfo> decodeAll(List<Object?> raw) =>
      raw.map((rawItem) => PeerInfo.decode(rawItem)).toList();

  @override
  String toString() =>
      '$runtimeType(addr: $addr, source: $source, state: $state, runtimeId: $runtimeId)';
}

class NetworkStats {
  final int bytesTx;
  final int bytesRx;
  final int throughputTx;
  final int throughputRx;

  const NetworkStats({
    this.bytesTx = 0,
    this.bytesRx = 0,
    this.throughputTx = 0,
    this.throughputRx = 0,
  });

  static NetworkStats decode(List<Object?> raw) => NetworkStats(
        bytesTx: raw[0] as int,
        bytesRx: raw[1] as int,
        throughputTx: raw[2] as int,
        throughputRx: raw[3] as int,
      );

  @override
  String toString() =>
      '$runtimeType(bytesTx: $bytesTx, bytesRx: $bytesRx, throughputTx: $throughputTx, throughputRx: $throughputRx)';
}

/// A handle to a Ouisync repository.
class Repository {
  final Client _client;
  final int _handle;
  final String _path;

  Repository._(this._client, this._handle, this._path);

  /// Creates a new repository and set access to it based on the following table:
  ///
  /// readSecret      |  writeSecret     |  token access  |  result
  /// ---------------------+------------------------+----------------+------------------------------
  /// null or any     |  null or any     |  blind         |  blind replica
  /// null            |  null or any     |  read          |  read without secret
  /// any             |  null or any     |  read          |  read with readSecret as secret
  /// null            |  null            |  write         |  read and write without secret
  /// any             |  null            |  write         |  read (only!) with secret
  /// null            |  any             |  write         |  read without secret, require secret for writing
  /// any             |  any             |  write         |  read with one secret, write with (possibly same) one
  static Future<Repository> create(
    Session session, {
    required String path,
    required SetLocalSecret? readSecret,
    required SetLocalSecret? writeSecret,
    ShareToken? token,
  }) async {
    final client = session._client;
    final handle = await client.invoke<int>(
      'repository_create',
      {
        'path': path,
        'read_secret': readSecret?.encode(),
        'write_secret': writeSecret?.encode(),
        'token': token?.toString(),
        'dht': false,
        'pex': false,
      },
    );

    final fullPath = await client.invoke<String>('repository_get_path', handle);

    return Repository._(client, handle, fullPath);
  }

  /// Opens an existing repository. If the same repository is opened again, a new handle pointing
  /// to the same underlying repository is returned.
  ///
  /// See also [close].
  static Future<Repository> open(
    Session session, {
    required String path,
    LocalSecret? secret,
  }) async {
    final client = session._client;
    final handle = await client.invoke<int>('repository_open', {
      'path': path,
      'secret': secret?.encode(),
    });

    final fullPath = await client.invoke<String>('repository_get_path', handle);

    return Repository._(client, handle, fullPath);
  }

  /// Returns all currently opened repositories.
  static Future<List<Repository>> list(Session session) => session._client
      .invoke<Map<Object?, Object?>>('repository_list')
      .then((handles) => handles
          .cast<String, int>()
          .entries
          .map((entry) => Repository._(session._client, entry.value, entry.key))
          .toList());

  /// Closes the repository. All outstanding handles become invalid. Invoking any operation on a
  /// repository after it's been closed results in an error being thrown.
  Future<void> close() async {
    await _client.invoke('repository_close', _handle);
  }

  String get path => _path;

  /// Checks whether syncing with other replicas is enabled.
  Future<bool> get isSyncEnabled =>
      _client.invoke<bool>('repository_is_sync_enabled', _handle);

  /// Enables or disables syncing with other replicas.
  Future<void> setSyncEnabled(bool enabled) =>
      _client.invoke('repository_set_sync_enabled', {
        'repository': _handle,
        'enabled': enabled,
      });

  /// Sets, unsets or changes local secrets for accessing the repository or disables the given
  /// access mode.
  Future<void> setAccess({
    AccessChange? read,
    AccessChange? write,
  }) =>
      _client.invoke('repository_set_access', {
        'repository': _handle,
        'read': read?.encode(),
        'write': write?.encode(),
      });

  /// Obtain the current repository credentials. They can be used to restore repository access
  /// (with [setCredentials]) after the repo has been closed and re-opened without needing the
  /// local secret. This is useful for example when renaming/moving the repository database.
  Future<Uint8List> get credentials =>
      _client.invoke<Uint8List>('repository_credentials', _handle);

  Future<void> setCredentials(Uint8List credentials) =>
      _client.invoke<void>('repository_set_credentials', {
        'repository': _handle,
        'credentials': credentials,
      });

  /// Like `setCredentials` but use the `ShareToken` and reset any values
  /// encrypted with local secrets to random values. Currently that is only the
  /// writer ID.
  Future<void> resetCredentials(ShareToken token) =>
      _client.invoke<void>('repository_reset_credentials', {
        'repository': _handle,
        'token': token.toString(),
      });

  Future<AccessMode> get accessMode {
    return _client
        .invoke<int>('repository_get_access_mode', _handle)
        .then((n) => AccessMode.decode(n));
  }

  Future<void> setAccessMode(AccessMode accessMode, {LocalSecret? secret}) =>
      _client.invoke('repository_set_access_mode', {
        'repository': _handle,
        'access_mode': accessMode.encode(),
        'secret': secret?.encode(),
      });

  /// Returns the type (file, directory, ..) of the entry at [path]. Returns `null` if the entry
  /// doesn't exists.
  Future<EntryType?> entryType(String path) async {
    final raw = await _client.invoke<int?>('repository_entry_type', {
      'repository': _handle,
      'path': path,
    });

    return raw != null ? EntryType.decode(raw) : null;
  }

  /// Returns whether the entry (file or directory) at [path] exists.
  Future<bool> entryExists(String path) async {
    return await entryType(path) != null;
  }

  /// Move/rename the file/directory from [src] to [dst].
  Future<void> moveEntry(String src, String dst) =>
      _client.invoke<void>('repository_move_entry', {
        'repository': _handle,
        'src': src,
        'dst': dst,
      });

  Stream<void> get events =>
      _client.subscribe('repository', _handle).cast<void>();

  Future<bool> get isDhtEnabled =>
      _client.invoke('repository_is_dht_enabled', _handle);

  Future<void> setDhtEnabled(bool enabled) =>
      _client.invoke('repository_set_dht_enabled', {
        'repository': _handle,
        'enabled': enabled,
      });

  Future<bool> get isPexEnabled =>
      _client.invoke<bool>('repository_is_pex_enabled', _handle);

  Future<void> setPexEnabled(bool enabled) =>
      _client.invoke<void>('repository_set_pex_enabled', {
        'repository': _handle,
        'enabled': enabled,
      });

  /// Create a share token providing access to this repository with the given mode.
  Future<ShareToken> share({
    required AccessMode accessMode,
    LocalSecret? secret,
  }) =>
      _client.invoke<String>('repository_share', {
        'repository': _handle,
        'secret': secret?.encode(),
        'mode': accessMode.encode(),
      }).then((token) => ShareToken._(_client, token));

  Future<Progress> get syncProgress => _client
      .invoke<List<Object?>>('repository_sync_progress', _handle)
      .then(Progress.decode);

  StateMonitor? get stateMonitor => StateMonitor.getRoot(_client)
      .child(MonitorId.expectUnique("Repositories"))
      .child(MonitorId.expectUnique(_path));

  Future<String> get infoHash =>
      _client.invoke<String>("repository_info_hash", _handle);

  Future<String> hexDatabaseId() async {
    final bytes =
        await _client.invoke<Uint8List>("repository_database_id", _handle);
    return HEX.encode(bytes);
  }

  /// Create mirror of this repository on the cache server.
  Future<void> createMirror(String host) =>
      _client.invoke<void>('repository_create_mirror', {
        'repository': _handle,
        'host': host,
      });

  /// Delete mirror of this repository from the cache server.
  Future<void> deleteMirror(String host) =>
      _client.invoke<void>('repository_delete_mirror', {
        'repository': _handle,
        'host': host,
      });

  /// Check if this repository is mirrored on the cache server.
  Future<bool> mirrorExists(String host) =>
      _client.invoke<bool>('repository_mirror_exists', {
        'repository': _handle,
        'host': host,
      });

  Future<PasswordSalt> getReadPasswordSalt() => _client
      .invoke<Uint8List>("get_read_password_salt", _handle)
      .then((bytes) => PasswordSalt(bytes));

  Future<PasswordSalt> getWritePasswordSalt() => _client
      .invoke<Uint8List>("get_write_password_salt", _handle)
      .then((bytes) => PasswordSalt(bytes));

  Future<String?> getMetadata(String key) =>
      _client.invoke<String?>('repository_get_metadata', {
        'repository': _handle,
        'key': key,
      });

  Future<bool> setMetadata(
    Map<String, ({String? oldValue, String? newValue})> edits,
  ) =>
      _client.invoke('repository_set_metadata', {
        'repository': _handle,
        'edits': edits.entries
            .map((entry) => {
                  'key': entry.key,
                  'old': entry.value.oldValue,
                  'new': entry.value.newValue,
                })
            .toList(),
      });

  /// Mount the repository if supported by the platform.
  Future<void> mount() => _client.invoke<void>('repository_mount', _handle);

  /// Unmount the repository.
  Future<void> unmount() => _client.invoke<void>('repository_unmount', _handle);

  /// Returns the mount point of this repository or null if not mounted.
  Future<String?> get mountPoint =>
      _client.invoke('repository_get_mount_point', _handle);

  /// Fetch the per-repository network statistics.
  Future<NetworkStats> get networkStats => _client
      .invoke<List<Object?>>('repository_stats', _handle)
      .then((list) => NetworkStats.decode(list));

  @override
  bool operator ==(Object other) =>
      other is Repository &&
      other._client == _client &&
      other._handle == _handle;

  @override
  int get hashCode => Object.hash(_client, _handle);

  @override
  String toString() => '$runtimeType($_path)';
}

sealed class AccessChange {
  Object? encode();
}

class EnableAccess extends AccessChange {
  final SetLocalSecret? secret;

  EnableAccess(this.secret);

  @override
  Object? encode() => {'enable': secret?.encode()};
}

class DisableAccess extends AccessChange {
  @override
  Object? encode() => 'disable';
}

class ShareToken {
  final Client _client;
  final String _token;

  ShareToken._(this._client, this._token);

  static Future<ShareToken> fromString(Session session, String s) =>
      session._client
          .invoke<String>('share_token_normalize', s)
          .then((s) => ShareToken._(session._client, s));

  /// Get the suggested repository name from the share token.
  Future<String> get suggestedName =>
      _client.invoke<String>('share_token_suggested_name', _token);

  Future<String> get infoHash =>
      _client.invoke<String>('share_token_info_hash', _token);

  /// Get the access mode the share token provides.
  Future<AccessMode> get mode => _client
      .invoke<int>('share_token_mode', _token)
      .then((n) => AccessMode.decode(n));

  /// Check if the repository of this share token is mirrored on the cache server.
  Future<bool> mirrorExists(String host) =>
      _client.invoke<bool>('share_token_mirror_exists', {
        'share_token': _token,
        'host': host,
      });

  @override
  String toString() => _token;

  @override
  bool operator ==(Object other) =>
      other is ShareToken && other._token == _token;

  @override
  int get hashCode => _token.hashCode;
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

  @override
  String toString() => '$name (${entryType.name})';
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

  /// Reads the directory of [repo] at [path].
  ///
  /// Throws if [path] doesn't exist or is not a directory.
  static Future<Directory> read(Repository repo, String path) async {
    final rawEntries = await repo._client.invoke<List<Object?>>(
      'directory_read',
      {
        'repository': repo._handle,
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
    return repo._client.invoke<void>('directory_create', {
      'repository': repo._handle,
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
    return repo._client.invoke<void>('directory_remove', {
      'repository': repo._handle,
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
  final Client _client;
  final int _handle;

  File._(this._client, this._handle);

  static const defaultChunkSize = 1024;

  /// Opens an existing file from [repo] at [path].
  ///
  /// Throws if [path] doesn't exists or is a directory.
  static Future<File> open(Repository repo, String path) async {
    return File._(
        repo._client,
        await repo._client.invoke<int>('file_open', {
          'repository': repo._handle,
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
        repo._client,
        await repo._client.invoke<int>('file_create', {
          'repository': repo._handle,
          'path': path,
        }));
  }

  /// Removes (deletes) a file at [path] from [repo].
  static Future<void> remove(Repository repo, String path) {
    return repo._client.invoke<void>('file_remove', {
      'repository': repo._handle,
      'path': path,
    });
  }

  /// Flushed and closes this file.
  Future<void> close() {
    return _client.invoke<void>('file_close', _handle);
  }

  /// Flushes any pending writes to this file.
  Future<void> flush() {
    return _client.invoke<void>('file_flush', _handle);
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

    return _client.invoke<Uint8List>(
        'file_read', {'file': _handle, 'offset': offset, 'len': size});
  }

  /// Write [data] to this file starting at [offset].
  Future<void> write(int offset, List<int> data) {
    if (debugTrace) {
      print("File.write");
    }

    return _client.invoke<void>('file_write', {
      'file': _handle,
      'offset': offset,
      'data': Uint8List.fromList(data),
    });
  }

  /// Truncate the file to [size] bytes.
  Future<void> truncate(int size) {
    if (debugTrace) {
      print("File.truncate");
    }

    return _client.invoke<void>('file_truncate', {
      'file': _handle,
      'len': size,
    });
  }

  /// Returns the length of this file in bytes.
  Future<int> get length {
    if (debugTrace) {
      print("File.length");
    }

    return _client.invoke<int>('file_len', _handle);
  }

  Future<int> get progress => _client.invoke<int>('file_progress', _handle);
}
