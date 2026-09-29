# Ouisync Kotlin bindings

This project provides kotlin bindings for the Ouisync library. It consist of these packages:

- **ouisync-session** is the entry point to Ouisync. It's used to manage the repositories, access
    their content and configure the sync protocol, among other things. Multiple *sessions* can
    connect to the same *service*, even across process boundaries.
- **ouisync-service-android** provides the Ouisync *service* which maintains the repositories and
    runs the sync protocol, for Android. It can be interacted with using *sessions*.
- **ouisync-service-jvm** provides the same *service* for desktop JVM (Linux, macOS and Windows,
    each on x86_64 and arm64).
- **ouisync-android** provides high-level components for developing Android apps:
    [foreground service](https://developer.android.com/develop/background-work/services/fgs) and
    [documents provider](https://developer.android.com/guide/topics/providers/document-provider#overview).

The two service packages contain the same API (from the **ouisync-service-common** package, which
they depend on and which is not meant to be used directly) and bundle the Ouisync native library
for their platform. Use only one of them in a given app - Gradle reports a capability conflict if
both end up in the same dependency graph.

## Installation

The packages are published on [Maven Central](https://central.sonatype.com/). Add them as
dependencies to your project.

Android:

```groovy
dependencies {
    implementation "ie.equalit.ouinet:ouisync-session:$ouisync_version"
    implementation "ie.equalit.ouinet:ouisync-service-android:$ouisync_version"

    // Optional
    implementation "ie.equalit.ouinet:ouisync-android:$ouisync_version"
}
```

Desktop JVM (requires Java 17 or newer):

```groovy
dependencies {
    implementation "ie.equalit.ouinet:ouisync-session:$ouisync_version"
    implementation "ie.equalit.ouinet:ouisync-service-jvm:$ouisync_version"
}
```

Kotlin Multiplatform (Android and desktop JVM targets):

```kotlin
kotlin {
    sourceSets {
        androidMain.dependencies {
            implementation("ie.equalit.ouinet:ouisync-session:$ouisyncVersion")
            implementation("ie.equalit.ouinet:ouisync-service-android:$ouisyncVersion")
        }

        jvmMain.dependencies {
            implementation("ie.equalit.ouinet:ouisync-session:$ouisyncVersion")
            implementation("ie.equalit.ouinet:ouisync-service-jvm:$ouisyncVersion")
        }
    }
}
```

Replace `$ouisync_version` with the version of Ouisync you want to use (all the packages always use
the same version).

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

(This example is for Android. On desktop JVM, use any directory the app can write to.)

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
this example we keep things simple and use only one `Session`.

Then we can configure the networking. Start by setting up the network listeners. We use the QUIC
protocol here. Ouisync supports both QUIC and TCP but QUIC generally works better. You can also
enable both. Binding to `0.0.0.0` makes it listen on all interfaces and port `0` means bind to a
random available port.

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
session.setStoreDirs(listOf(context.getDir("ouisync-repos").getPath()))
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

A simple Android example app is in the
[bindings/kotlin/example](https://github.com/equalitie/ouisync/tree/master/bindings/kotlin/example)
folder. To build it run `./gradlew example:assembleDebug`. Find the apk in
`build/example/outputs/apk/debug/example-debug.apk`, install and run it on a device or an emulator.

## Build from source

### Prerequisites

- [Rust toolchain](https://www.rust-lang.org/learn/get-started). The easiest way to get it is using
  [rustup](https://rustup.rs/).
- JDK 17 or newer.
- Android SDK.
- For the Android packages: the Android NDK in the version specified in
  [ndk-version.txt](../../ndk-version.txt), [cargo-ndk](https://github.com/bbqsrc/cargo-ndk)
  (`cargo install cargo-ndk`) and the rust targets for the Android ABIs
  (`rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`).
- For the release build of `ouisync-service-jvm`: `llvm-strip`, used to strip the native library
  (`rustup component add llvm-tools`).
- For cross-compiling `ouisync-service-jvm` for other platforms than the host:
  [cross](https://github.com/cross-rs/cross) (or `cargo` with the corresponding rust target and
  toolchain).

### Build packages

Run from inside the `bindings/kotlin` folder, e.g.:

- `./gradlew assemble` to build all packages.
- `./gradlew :ouisync-service-android:assembleDebug` to build a single package (here the debug
  variant of `ouisync-service-android`).
- `./gradlew publishToMavenLocal` to publish the packages to the local Maven repository.
- `./gradlew test` to run the tests.

To see other available tasks, run `./gradlew tasks`.

Only the release builds of the packages are published to Maven Central. Debug builds (with the
native library built using the debug cargo profile) can be built and used locally: the debug
variants of the Android packages, and `ouisync-service-jvm` with `-Pouisync.profile=debug`.

### Build properties

The build of the native libraries can be configured using the following Gradle properties (pass
them with `-P<name>=<value>`, e.g.
`./gradlew :ouisync-service-android:assembleRelease -Pouisync.targets=aarch64-linux-android`):

| Property                | Description |
| ----------------------- | ----------- |
| `ouisync.targets`       | Comma separated list of rust target triples to build the native library for. Android targets are used by `ouisync-service-android`, the others by `ouisync-service-jvm` (so a single list can contain targets for both). A package for which the list contains no targets fails to build. Default: all Android targets and the host (desktop) target. |
| `ouisync.cargo`         | Tool to build the non-host `ouisync-service-jvm` targets with: `cross` (default) or `cargo`. |
| `ouisync.profile`       | Cargo profile for `ouisync-service-jvm`: `release` (default) or `debug`. |
| `ouisync.nativeLibsDir` | Build `ouisync-service-jvm` using the prebuilt native libraries from this directory instead of building them. It must have the layout `<platform>/<library>`, e.g. `linux-x86-64/libouisync_service.so`. |
| `target-platform`       | Comma separated list of Flutter target platforms (e.g., `android-arm64`) to build `ouisync-service-android` for. Passed by Flutter; can't be combined with `ouisync.targets`. |

By default, `ouisync-service-jvm` contains the native library for the host platform only. The
published package contains the libraries for all the supported platforms, which are built on CI
and packaged using `ouisync.nativeLibsDir`.
