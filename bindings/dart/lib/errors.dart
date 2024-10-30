import 'package:flutter/services.dart';


/* Thrown when a native call could not be completed because the
flutter application host is itself shutting down. Usually when
this happens, the dart vm is probably shutting down as well.

Corresponds to error code OS00 */
class HostShutdown extends Error {}

/* Thrown when the application host encountered an unexpected error
while processing the request. This differs from BindingsOutOfDate
by the host explicitly notifying us of this situation.

Corresponds to error code OS01 */
class InternalError extends Error {}

/* Thrown when the application host was unable to open a channel
to the rust library. The usual way to handle this is to show it
to the user and try to create a new session at a later time.

Corresponds to error code OS02 */
class ProviderUnavailable implements Exception {}

/* Thrown when the application host was unable to collect the
arguments of a method call. This likely means that an argument
was not provided when it was required by a native call.

Corresponds to error code OS03 */
class HostArgumentError extends ArgumentError {}

/* Thrown for every request made after the session has been closed. 

Corresponds to error code OS04 */
class SessionClosed extends StateError {
  SessionClosed() : super('Session closed');
}

/* Thrown if the application host attempts to communicate with the
library host (sometimes called file provider extension) over an
unsupported channel. Currently this is specific to macOS and there
is no reasonable way to handle it other than craashing.

Corresponds to error code OS05 */
class HostLinkingError extends UnsupportedError {
  HostLinkingError(super.message);
}

/* Thrown if the bindings attempt to invoke a method that is not
exported by the application host. De facto equivalent with
NoSuchMethodError, but presently without the stack information.

Corresponds to error code OS06 */
class MethodNotExported extends UnsupportedError {
  MethodNotExported(super.message);
}

/* Thrown when an error was thrown with an associated error code
that was not recognized by this version of the package. While you
can inspect the error code and message manually, it's probably
better to update the bindings or report this.

Corresponds to any other error code not described here */
class BindingsOutOfDate extends UnimplementedError {
  final String code;
  BindingsOutOfDate(this.code, super.message);
}

Object mapError(PlatformException err) => switch (err.code) {
  'OS00' => HostShutdown(),
  'OS01' => InternalError(),
  'OS02' => ProviderUnavailable(),
  'OS03' => HostArgumentError(),
  'OS04' => SessionClosed(),
  'OS06' => HostLinkingError(err.message!),
  'OS07' => MethodNotExported(err.message!),
  _ => BindingsOutOfDate(err.code, err.message)
};