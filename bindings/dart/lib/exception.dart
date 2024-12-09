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
        ErrorCode.invalidData => InvalidData(message, sources),
        ErrorCode.serviceAlreadyRunning =>
          ServiceAlreadyRunning(message, sources),
        _ => OuisyncException._(code, message, sources),
      };

  @override
  String toString() => [message].followedBy(sources).join(' → ');
}

class InvalidData extends OuisyncException {
  InvalidData([String? message, List<String> sources = const []])
      : super._(ErrorCode.invalidData, message, sources);
}

class ServiceAlreadyRunning extends OuisyncException {
  ServiceAlreadyRunning([String? message, List<String> sources = const []])
      : super._(ErrorCode.serviceAlreadyRunning, message, sources);
}
