# Ouisync Kotlin bindings

This project provides kotlin bindings for the Ouisync library. It consist of these packages:

- **ouisync-service** provides the Ouisync *service* which maintains the repositories and runs the
    sync protocol. It can be interacted with using *sessions*.
- **ouisync-session** is the entry point to Ouisync. It's used to manage the repositories, access
    their content and configure the sync protocol, among other things. Multiple *sessions* can
    connect to the same *service*, even across process boundaries.
- **ouisync-android** provides high-level components for developing Android apps:
    [foreground service](https://developer.android.com/develop/background-work/services/fgs) and
    [documents provider](https://developer.android.com/guide/topics/providers/document-provider#overview).

## Installation

The packages are published on [Maven Central](https://central.sonatype.com/). Add them as
dependencies to your project:

```groovy
dependencies {
    implementation "ie.equalit.ouinet:ie.equalit.ouinet:ouisync-session:$ouisync_version"
    implementation "ie.equalit.ouinet:ie.equalit.ouinet:ouisync-service:$ouisync_version"
    implementation "ie.equalit.ouinet:ie.equalit.ouinet:ouisync-android:$ouisync_version"
}
```

Replace `$ouisync_version` with the version of Ouisync you want to use (all three packages always
use the same version).

## Getting started

🗈 Note that almost all functions in these bindings are `suspend` and so need to be invoked in appropriate
coroutine scope.

First, create and start a `Service`. This is the main component of Ouisync which is responsible for
maintaining the repositories and running the sync protocol. It requires a directory in which to
store its configuration files:

```kotlin
val configDir = context.getDir("ouisync-config").getPath()
val service = Service.start(configDir)
```

On app shutdown, stop the service to allow it to close all repositories and peer connections:

```kotlin
service.stop()
```

In order to perform any actions, we need to create a `Session`, which is the entry point to Ouisync.
Pass it the same configuration directory as the Service:

```kotlin
val session = Session.create(configDir)
```

🗈 Note there can be multiple `Session`s per `Service`. This is useful for example when the app
consist of multiple components that all need to access the same set of Ouisync repositories. In
this example we keep things simple and use only once `Session`.


Then we can configure the networking. Start by setting up the network listeners. We use the QUIC protocol here. Ouisync supports both QUIC and TCP but QUIC generally works better. You can also enable both. Binding to `0.0.0.0` makes it listen on all interfaces and port `0` means bind to a random available port.

```kotlin
session.bindNetwork(listOf("quic:0.0.0.0:0"))
```

Then we can set up local discovery to find peers on the local network (Later we'll show how to
discover peers on the internet as well):

```kotlin
session.setLocalDiscoveryEnabled(true)
```

To create repositories, we first need to specify the directory (or directories) in which the
repository data will be stored:

```kotlin
session.setStorDirs(listOf(context.getDir("ouisync-repos").getPath()))
```

Then we can actually create the repository:

```kotlin
val repo = session.createRepository("my-repo")
```

By default the repository will not sync so we need to enable it. We'll also enable more peer
discovery options: DHT and Peer Exchange. Note that these configurations only need to be done once
as they will be persisted in the repository.

```kotlin
repo.setSyncEnabled(true)
repo.setDhtEnabled(true)
repo.setPexEnabled(true)
```

We can also retrieve the list of existing repositories. This returns a `Map` where the keys are the
repository names and the values are the corresponding `Repository` objects

```kotlin
val repos = session.listRepositories()
```

Finally we'll show how to access repository content. For more info refer to the API documentation.

```kotlin
// Create a file and write into it
val file = repo.createFile("hello.txt")
file.write(0, "Hello world\n".toByteArray())
file.flush()
file.close()

// Create a directory
repo.createDirectory("docs")

// Move the file to the directory
repo.moveEntry("hello.txt", "docs/hello.txt")

// And so on...
```

## API documentation

Documentation is available at [docs.ouisync.net](https://docs.ouisync.net/kotlin).

## Examples

A simple example app is in the
[bindings/kotlin/example](https://github.com/equalitie/ouisync/tree/master/bindings/kotlin/example)
folder. To build it run `gradle example:assembleDebug`. Find the apk in
`build/example/outputs/apk/debug/example-debug.apk`, install and run it on a device or an emulator.

## Build from source

### Prerequisities

The Ousiync native library is built automatically but it requires a rust toolchain. The easiest way
to get it is using [rustup](https://rustup.rs/).

### Build packages

To build all packages in all variants (release, debug), run `gradle assembleRelease` from inside the
`bindings/kotlin` folder. To see other available tasks, run `gradle tasks`.
