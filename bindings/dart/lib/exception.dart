import 'bindings.dart';
export 'bindings.dart' show ErrorCode;

/// The exception type throws from this library.
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
        ErrorCode.alreadyExists => AlreadyExists(message, sources),
        ErrorCode.invalidData => InvalidData(message, sources),
        ErrorCode.listenerBind => ServiceAlreadyRunning(message, sources),
        ErrorCode.notFound => NotFound(message, sources),
        ErrorCode.permissionDenied => PermissionDenied(message, sources),
        ErrorCode.serviceAlreadyRunning =>
          ServiceAlreadyRunning(message, sources),
        ErrorCode.unsupported => Unsupported(message, sources),
        ErrorCode.vfsDriverInstallError =>
          VFSDriverInstallError(message, sources),
        _ => OuisyncException._(code, message, sources),
      };

  @override
  String toString() => [message].followedBy(sources).join(' → ');
}

class AlreadyExists extends OuisyncException {
  AlreadyExists([String? message, List<String> sources = const []])
      : super._(ErrorCode.alreadyExists, message, sources);
}

class InvalidData extends OuisyncException {
  InvalidData([String? message, List<String> sources = const []])
      : super._(ErrorCode.invalidData, message, sources);
}

class NotFound extends OuisyncException {
  NotFound([String? message, List<String> sources = const []])
      : super._(ErrorCode.notFound, message, sources);
}

class PermissionDenied extends OuisyncException {
  PermissionDenied([String? message, List<String> sources = const []])
      : super._(ErrorCode.permissionDenied, message, sources);
}

class ServiceAlreadyRunning extends OuisyncException {
  ServiceAlreadyRunning([String? message, List<String> sources = const []])
      : super._(ErrorCode.serviceAlreadyRunning, message, sources);
}

class Unsupported extends OuisyncException {
  Unsupported([String? message, List<String> sources = const []])
      : super._(ErrorCode.unsupported, message, sources);
}

class VFSDriverInstallError extends OuisyncException {
  VFSDriverInstallError([String? message, List<String> sources = const []])
      : super._(ErrorCode.vfsDriverInstallError, message, sources);
}
