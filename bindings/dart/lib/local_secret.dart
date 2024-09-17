part of 'ouisync.dart';

// Used for opening a reository.
sealed class LocalSecret {
  Object? encode();

  @override
  String toString();
}

// Used for creating a reository and changing its local secret.
sealed class SetLocalSecret {
  Object? encode();

  LocalSecret toLocalSecret();

  @override
  String toString();
}

class LocalPassword implements LocalSecret, SetLocalSecret {
  final String string;

  LocalPassword(this.string);

  int get length => string.length;
  bool get isEmpty => string.isEmpty;

  @override
  LocalSecret toLocalSecret() {
    return this;
  }

  @override
  Object? encode() => {'password': string};

  // Discourage from writing local secret into the log.
  @override
  String toString() => "LocalPassword(***)";

  @override
  bool operator ==(Object other) {
    if (other is! LocalPassword) return false;
    return string == other.string;
  }
}

class LocalSecretKey implements LocalSecret {
  // 256-bit as used by the Rust ChaCha20 implementation Ouisync is using.
  static const sizeInBytes = 32;

  final Uint8List _bytes;

  LocalSecretKey(this._bytes);

  Uint8List get bytes => _bytes;

  static LocalSecretKey random() {
    return LocalSecretKey(_randomBytes(sizeInBytes));
  }

  @override
  Object? encode() => {'secret_key': _bytes};

  // Discourage from writing local secret into the log.
  @override
  String toString() => "LocalSecretKey(***)";
}

class LocalSecretKeyAndSalt implements SetLocalSecret {
  // 256-bit as used by the Rust ChaCha20 implementation Ouisync is using.
  static const sizeInBytes = 32;

  final LocalSecretKey key;
  final PasswordSalt salt;

  LocalSecretKeyAndSalt(this.key, this.salt);

  static LocalSecretKeyAndSalt random() =>
      LocalSecretKeyAndSalt(LocalSecretKey.random(), PasswordSalt.random());

  @override
  LocalSecret toLocalSecret() {
    return key;
  }

  @override
  Object? encode() => {
        'key_and_salt': {'key': key.bytes, 'salt': salt.bytes}
      };

  // Discourage from writing local secret into the log.
  @override
  String toString() => "LocalSecretKeyAndSalt(***, $salt)";
}

class PasswordSalt {
  // https://docs.rs/argon2/latest/argon2/constant.RECOMMENDED_SALT_LEN.html
  static const sizeInBytes = 16;

  final Uint8List _bytes;

  PasswordSalt(this._bytes);

  Uint8List get bytes => _bytes;

  static PasswordSalt random() {
    return PasswordSalt(_randomBytes(sizeInBytes));
  }

  String toBase64() => base64.encode(_bytes);

  static PasswordSalt fromBase64(String salt64) =>
      PasswordSalt(base64.decode(salt64));

  // Discourage from writing local secret into the log.
  @override
  String toString() => "PasswordSalt(${base64.encode(_bytes)})";
}

Uint8List _randomBytes(int size) {
  final random = Random.secure();
  Uint8List bytes = Uint8List(size);
  for (int i = 0; i < size; i++) {
    bytes[i] = random.nextInt(256);
  }
  return bytes;
}
