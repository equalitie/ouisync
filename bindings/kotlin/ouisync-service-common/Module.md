# Module ouisync-service-common

Provides the [Service][org.equalitie.ouisync.service.Service] class which maintains the
repositories and runs the sync protocol.

This package contains only the platform independent Kotlin code and is not meant to be used
directly. Use `ouisync-service-android` (Android) or `ouisync-service-jvm` (desktop JVM) instead,
which depend on this package and additionally bundle the native library for their platform.

To interact with a `Service`, use one or more [Session][org.equalitie.ouisync.session.Session]s from
the `ouisync-session` package. The `Session`(s) must use the same *config directory* as the
`Service`.

## Example

```kotlin
import org.equalitie.ouisync.service.Service
import org.equalitie.ouisync.session.Session
import org.equalitie.ouisync.session.close
import org.equalitie.ouisync.session.create

// All methods of `Service` and `Session` are `suspend`, so they need to be called from a coroutine.
suspend fun run(configDir: String) {
    val service = Service.start(configDir)

    // Use the same config dir as the service in order to connect to it.
    val session = Session.create(configDir)

    try {
        // Use the session ...
    } finally {
        session.close()
        service.stop()
    }
}
```
