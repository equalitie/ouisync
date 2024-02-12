part of 'ouisync_plugin.dart';

sealed class LocalSecret {
  Object? encode();

  @override
  String toString();
}

class LocalPassword extends LocalSecret {
  final String password;

  LocalPassword(this.password);

  @override
  Object? encode() => {'password': password};

  // Discourage from writing local secret into the log.
  @override
  String toString() => "LocalPassword(***)";
}

class LocalSecretKey extends LocalSecret {
  final Uint8List _key;

  LocalSecretKey._(this._key);

  String toBase64() => base64.encode(_key);

  @override
  Object? encode() => {'secret_key': _key};

  // Discourage from writing local secret into the log.
  @override
  String toString() => "LocalSecretKey(***)";
}

class PasswordSalt {
  final Uint8List _salt;

  PasswordSalt._(this._salt);

  String toBase64() => base64.encode(_salt);

  // Discourage from writing local secret into the log.
  @override
  String toString() => "PasswordSalt(***)";
}
