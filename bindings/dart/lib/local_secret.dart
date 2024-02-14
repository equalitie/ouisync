part of 'ouisync_plugin.dart';

sealed class LocalSecret {
  Object? encode();

  @override
  String toString();
}

class LocalPassword extends LocalSecret {
  final String string;

  LocalPassword(this.string);

  int get length => string.length;
  bool get isEmpty => string.isEmpty;

  @override
  Object? encode() => {'password': string};

  // Discourage from writing local secret into the log.
  @override
  String toString() => "LocalPassword(***)";
}

class LocalSecretKey extends LocalSecret {
  // 256-bit as used by the Rust ChaCha20 implementation Ouisync is using.
  static const sizeInBytes = 32;

  final Uint8List _bytes;

  LocalSecretKey(this._bytes);

  Uint8List get bytes => _bytes;

  static LocalSecretKey generateRandom() {
    final random = Random.secure();
    Uint8List bytes = Uint8List(sizeInBytes);
    for (int i = 0; i < sizeInBytes; i++) {
      bytes[i] = random.nextInt(256);
    }
    return LocalSecretKey(bytes);
  }

  @override
  Object? encode() => {'secret_key': _bytes};

  // Discourage from writing local secret into the log.
  @override
  String toString() => "LocalSecretKey(***)";
}

class PasswordSalt {
  final Uint8List _bytes;

  PasswordSalt(this._bytes);

  Uint8List get bytes => _bytes;

  String toBase64() => base64.encode(_bytes);

  static PasswordSalt fromBase64(String salt64) =>
      PasswordSalt(base64.decode(salt64));

  // Discourage from writing local secret into the log.
  @override
  String toString() => "PasswordSalt(***)";
}
