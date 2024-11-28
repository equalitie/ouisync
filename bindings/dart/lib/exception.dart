import 'bindings.dart';
export 'bindings.dart' show ErrorCode;

/// The exception type throws from this library.
class OuisyncException implements Exception {
  final ErrorCode code;
  final String message;

  OuisyncException._(this.code, String? message)
      : message = message ?? code.toString() {
    assert(code != ErrorCode.ok);
  }

  factory OuisyncException(ErrorCode code, [String? message]) => switch (code) {
        ErrorCode.serviceAlreadyRunning => ServiceAlreadyRunning(message),
        _ => OuisyncException._(code, message),
      };

  @override
  String toString() => message;
}

class ServiceAlreadyRunning extends OuisyncException {
  ServiceAlreadyRunning([String? message])
      : super._(ErrorCode.serviceAlreadyRunning, message);
}
