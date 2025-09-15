# Module ouisync-service

Provides the [Service][org.equalitie.ouisync.service.Service] class which maintains the
repositories and runs the sync protocol.

This package contains native libraries written in Rust and needs a [rust toolchain]
(https://www.rust-lang.org/learn/get-started) to be installed on the system in order to build
them.

To interact with a `Service`, use one or more [Session][org.equalitie.ouisync.session.Session]s from
the `ouisync-session` package. The `Session`(s) must use the same *config directory* as the
`Service`.

## Example

```kotlin
import android.app.Activity
import android.os.Bundle
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.equalitie.ouisync.service.Service
import org.equalitie.ouisync.service.Session
import org.equalitie.ouisync.service.close
import org.equalitie.ouisync.service.create

class MyActivity : Activity() {
    // All methods of `Service` and `Session` are `suspend`, so we need a coroutine scope.
    private val scope = CoroutineScope(Dispatchers.Main)
    private var service: Service?
    private var session: Session?

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(bundle)

        val configDir = getDir("config")

        scope.launch {
            service = Service.start(configDir)

            // Use the same config dir as the service in order to connect to it.
            session = Session.create(configDir)
        }
    }

    override fun onDestroy() {
        scope.launch {
            session?.close()
            service?.stop()
        }
    }

    // ...
}
```
