# Module ouisync

## Quick start

The entry point to Ouisync is the [Session](org.equalitie.ouisync.lib.Session) class. [Create]
(org.equalitie.ouisync.lib.Session.Companion.create) it near the start of the app and make effort
to [close](org.equalitie.ouisync.lib.Session.close) it on app shutdown. Afterwards, use the
[Repository](org.equalitie.ouisync.lib.Repository) class to create and manage your Ouisync
repositories and use the [File](org.equalitie.ouisync.lib.File) and [Directory]
(org.equalitie.ouisync.lib.Directory) classes to access the repository content.

See also the [example app](https://github.com/equalitie/ouisync/tree/master/bindings/kotlin/example).