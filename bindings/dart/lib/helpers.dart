import 'package:flutter/services.dart';

final MethodChannel _channel =
    const MethodChannel('org.equalitie.ouisync.plugin');

/// View file at the given url using external app.
///
/// Returns `true` on success and `false` if there was no corresponding app to handle the url.
///
/// Note: currently this function is implemented only on android. Use the `url_launcher` package on
/// other platforms in the meantime.
Future<bool> viewFile(Uri uri, {String? chooserTitle}) => _channel
    .invokeMethod<bool>('viewFile', uri.toString())
    .then((result) => result ?? false);

/// Share file at the given url.
///
/// Note: Currently this function is implemented only on android. There is no alternative on other
/// platforms yet.
Future<void> shareFile(Uri uri) =>
    _channel.invokeMethod('shareFile', uri.toString());
